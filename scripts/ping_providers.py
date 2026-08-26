#!/usr/bin/env python3
"""Probe every sender endpoint in sniper.json the way the sender itself does.

The sniper keeps one warm TCP connection per regional endpoint and holds it open with a
periodic GET to that provider's `health_path`. Two things can go wrong quietly, and neither
shows up as an error at runtime:

  * a provider has no `health_path`, so it is never probed and its connection goes cold —
    every launch then pays a TCP handshake on the hot path;
  * a provider answers the probe but hangs up anyway, because its idle window is shorter
    than the sniper's probe interval. helius-sender closes at TEN seconds, which is how
    sender.rs KEEPALIVE_SECS ended up at 6.

So this does not just check that a host is reachable. It opens one connection, sends two
probes on it spaced apart, and reports whether the second one still worked. That is the
property the sniper actually depends on.

A non-200 is not a failure. jito and nextblock 404 every path and that is fine — the probe
exists to get *a* reply on a connection the server leaves open, not to authenticate.

    python scripts/ping_providers.py                    # one probe per endpoint
    python scripts/ping_providers.py --reuse-after 8    # what the running sniper does
    python scripts/ping_providers.py --reuse-after 10   # helius-sender should drop here
    python scripts/ping_providers.py --provider nozomi --config path.json

Exits non-zero if any endpoint could not be reached or dropped the connection.
"""

import argparse
import concurrent.futures as futures
import json
import os
import socket
import ssl
import sys
import time

# how long the sniper waits before probing (sender.rs KEEPALIVE_SECS) plus its ~2s spin window
SNIPER_PROBE_SECS = 8

# Idle windows in seconds. helius-sender and flashblock were MEASURED with --reuse-after
# (helius survives 9s and is gone at 10s); nozomi's 65s is documented. Anything at or below
# SNIPER_PROBE_SECS means that provider reconnects on the hot path.
IDLE_WINDOWS = {"helius-sender": 10, "flashblock": 30, "nozomi": 65}

RESET, BOLD, DIM = "\033[0m", "\033[1m", "\033[2m"
RED, GREEN, YELLOW = "\033[31m", "\033[32m", "\033[33m"


def colour(s, c):
    return s if os.environ.get("NO_COLOR") else f"{c}{s}{RESET}"


class Probe:
    """One endpoint, held open across probes exactly like the sender holds it."""

    def __init__(self, provider, host, port, tls, health_path, headers, timeout):
        self.provider = provider
        self.host = host
        self.port = port
        self.tls = tls
        self.health_path = health_path
        self.headers = headers
        self.timeout = timeout
        self.sock = None

    def connect(self):
        t0 = time.perf_counter()
        raw = socket.create_connection((self.host, self.port), timeout=self.timeout)
        raw.setsockopt(socket.IPPROTO_TCP, socket.TCP_NODELAY, 1)
        if self.tls:
            ctx = ssl.create_default_context()
            raw = ctx.wrap_socket(raw, server_hostname=self.host)
        self.sock = raw
        return (time.perf_counter() - t0) * 1000, raw.getpeername()[0]

    def request(self):
        """Send one keep-alive probe. Returns (status, ms, server_wants_close)."""
        req = [f"GET {self.health_path} HTTP/1.1", f"Host: {self.host}", "Connection: keep-alive"]
        req += [f"{k}: {v}" for k, v in self.headers]
        wire = ("\r\n".join(req) + "\r\n\r\n").encode()

        t0 = time.perf_counter()
        self.sock.sendall(wire)
        chunk = self.sock.recv(4096)
        ms = (time.perf_counter() - t0) * 1000
        if not chunk:
            raise ConnectionError("server closed without replying")

        head = chunk.split(b"\r\n\r\n", 1)[0].decode("latin1")
        status = head.split("\r\n", 1)[0].split(" ")
        status = int(status[1]) if len(status) > 1 and status[1].isdigit() else 0
        wants_close = any(
            line.lower().startswith("connection:") and "close" in line.lower()
            for line in head.split("\r\n")
        )
        return status, ms, wants_close

    def close(self):
        if self.sock:
            try:
                self.sock.close()
            except OSError:
                pass


def check(ep, reuse_after):
    """Connect, probe, optionally wait and probe again on the SAME connection."""
    out = dict(provider=ep.provider, host=ep.host, ok=False, note="")
    try:
        out["connect_ms"], out["ip"] = ep.connect()
        status, ms, wants_close = ep.request()
        out.update(status=status, ms=ms, ok=True)
        if wants_close:
            out["ok"] = False
            out["note"] = "server sent Connection: close — will not stay warm"
            return out

        if reuse_after:
            time.sleep(reuse_after)
            try:
                status2, ms2, _ = ep.request()
                out["reuse_status"], out["reuse_ms"] = status2, ms2
            except Exception as e:  # noqa: BLE001 — any failure here is the finding
                out["ok"] = False
                out["note"] = f"connection died within {reuse_after}s: {type(e).__name__}"
    except Exception as e:  # noqa: BLE001
        out["note"] = f"{type(e).__name__}: {e}"
    finally:
        ep.close()
    return out


def load(config):
    with open(config, encoding="utf-8") as fh:
        cfg = json.load(fh)

    eps, skipped = [], []
    for p in cfg.get("providers", []):
        if not p.get("enabled", True):
            continue
        name = p["name"]
        health = p.get("health_path", "")
        hosts = p.get("hosts") or ([p["host"]] if p.get("host") else [])
        if not health:
            skipped.append((name, len(hosts)))
            continue
        for h in hosts:
            # a host may carry its own path suffix; the health path is still provider-wide
            host = h.split("/", 1)[0]
            eps.append(
                dict(
                    provider=name,
                    host=host,
                    port=p.get("port", 80),
                    tls=p.get("tls", False),
                    health_path=health,
                    headers=[tuple(x) for x in p.get("headers", [])],
                )
            )
    return eps, skipped


def main():
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("--config", default=os.path.join("shred-sniper", "sniper.json"))
    ap.add_argument("--timeout", type=float, default=8.0)
    ap.add_argument(
        "--reuse-after",
        type=float,
        default=0,
        metavar="SECS",
        help="hold each connection this long and probe again, proving it survives the gap. "
        "8 is what the running sniper does; 10 kills helius-sender, 31 kills flashblock.",
    )
    ap.add_argument("--provider", help="only this provider")
    args = ap.parse_args()

    eps, skipped = load(args.config)
    if args.provider:
        eps = [e for e in eps if e["provider"] == args.provider]
    if not eps:
        print(f"no endpoints to probe in {args.config}", file=sys.stderr)
        return 2

    note = f", holding each connection {args.reuse_after:g}s" if args.reuse_after else ""
    print(f"{BOLD}probing {len(eps)} endpoints from {args.config}{note}{RESET}\n")

    probes = [Probe(timeout=args.timeout, **e) for e in eps]
    # one thread per endpoint: with --reuse-after they all wait concurrently, so the run
    # takes about reuse_after seconds rather than reuse_after * len(eps)
    with futures.ThreadPoolExecutor(max_workers=min(64, len(probes))) as pool:
        results = list(pool.map(lambda p: check(p, args.reuse_after), probes))

    failures, by_provider = [], {}
    for r in results:
        by_provider.setdefault(r["provider"], []).append(r)

    for provider, rows in by_provider.items():
        window = IDLE_WINDOWS.get(provider)
        warn = ""
        if window is not None and window <= SNIPER_PROBE_SECS:
            warn = colour(f"  [idle window {window}s <= probe {SNIPER_PROBE_SECS}s!]", RED)
        elif window is not None:
            warn = colour(f"  [idle window {window}s]", DIM)
        print(f"{BOLD}{provider}{RESET}{warn}")
        for r in sorted(rows, key=lambda x: x.get("connect_ms", 9e9)):
            if not r["ok"]:
                failures.append(r)
                print(f"  {colour('FAIL', RED)}  {r['host']:<46} {r['note']}")
                continue
            reuse = ""
            if "reuse_status" in r:
                reuse = f"  reuse {r['reuse_status']} {r['reuse_ms']:.0f}ms"
            status = r["status"]
            tag = colour(f"{status}", GREEN if status == 200 else YELLOW)
            print(
                f"  {colour('ok', GREEN)}    {r['host']:<46} {tag:<14}"
                f" connect {r['connect_ms']:6.1f}ms  probe {r['ms']:6.1f}ms{reuse}"
                f"  {DIM}{r.get('ip','')}{RESET}"
            )
        print()

    if skipped:
        print(colour("providers with NO health_path (never probed, connections go cold):", RED))
        for name, n in skipped:
            print(f"  {name} ({n} endpoints)")
        print()

    total = len(results)
    print(f"{BOLD}{total - len(failures)}/{total} endpoints healthy{RESET}")
    if not args.reuse_after:
        print(
            f"{DIM}non-200 is fine - jito and nextblock 404 by design. Re-run with "
            f"--reuse-after 8 to prove connections survive the probe interval.{RESET}"
        )
    return 1 if failures or skipped else 0


if __name__ == "__main__":
    sys.exit(main())
