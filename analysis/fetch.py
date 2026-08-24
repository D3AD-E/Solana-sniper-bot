"""Phase 1+2: pull wallet signatures and raw transactions from Helius."""
import json, os, re, sys, time, threading
from concurrent.futures import ThreadPoolExecutor
import requests

ROOT = os.path.dirname(os.path.abspath(__file__))
DATA = os.path.join(ROOT, "data")
os.makedirs(DATA, exist_ok=True)

def rpc_url():
    env = open(os.path.join(ROOT, "..", ".env"), encoding="utf-8", errors="ignore").read()
    m = re.search(r"RPC_SLOW_ENDPOINT=(\S+)", env)
    return m.group(1).strip()

URL = rpc_url()
S = requests.Session()
S.headers["Content-Type"] = "application/json"
_lock = threading.Lock()

def post(payload, tries=6):
    for i in range(tries):
        try:
            r = S.post(URL, json=payload, timeout=60)
            if r.status_code == 429:
                time.sleep(1.5 * (i + 1)); continue
            r.raise_for_status()
            return r.json()
        except Exception as e:
            if i == tries - 1:
                raise
            time.sleep(1.5 * (i + 1))

def call(method, params):
    return post({"jsonrpc": "2.0", "id": 1, "method": method, "params": params}).get("result")

def get_signatures(addr, max_sigs=100000, before=None):
    out, seen = [], set()
    while len(out) < max_sigs:
        p = {"limit": 1000}
        if before:
            p["before"] = before
        res = call("getSignaturesForAddress", [addr, p])
        if not res:
            break
        for r in res:
            if r["signature"] not in seen:
                seen.add(r["signature"]); out.append(r)
        before = res[-1]["signature"]
        print(f"  sigs={len(out)} slot={res[-1]['slot']} t={res[-1].get('blockTime')}", flush=True)
        if len(res) < 1000:
            break
    return out[:max_sigs]

def batch_txs(sigs, workers=6, chunk=50):
    chunks = [sigs[i:i + chunk] for i in range(0, len(sigs), chunk)]
    done = [0]
    def work(ch):
        payload = [{"jsonrpc": "2.0", "id": i, "method": "getTransaction",
                    "params": [s, {"maxSupportedTransactionVersion": 0, "encoding": "jsonParsed"}]}
                   for i, s in enumerate(ch)]
        res = post(payload)
        out = [None] * len(ch)
        for item in res:
            out[item["id"]] = item.get("result")
        with _lock:
            done[0] += len(ch)
            print(f"  txs {done[0]}/{len(sigs)}", end="\r", flush=True)
        return out
    got = []
    with ThreadPoolExecutor(workers) as ex:
        for r in ex.map(work, chunks):
            got.extend(r)
    print()
    return got

if __name__ == "__main__":
    addr = sys.argv[1]
    max_sigs = int(sys.argv[2]) if len(sys.argv) > 2 else 20000
    sig_path = os.path.join(DATA, f"{addr}_sigs.json")
    if os.path.exists(sig_path):
        sigs = json.load(open(sig_path))
        print(f"cached sigs: {len(sigs)}")
    else:
        print("fetching signatures...")
        sigs = get_signatures(addr, max_sigs)
        json.dump(sigs, open(sig_path, "w"))
    ok = [s for s in sigs if s.get("err") is None]
    print(f"total={len(sigs)} success={len(ok)} failed={len(sigs)-len(ok)}")
    tx_path = os.path.join(DATA, f"{addr}_txs.jsonl")
    have = set()
    if os.path.exists(tx_path):
        for line in open(tx_path, encoding="utf-8"):
            try:
                have.add(json.loads(line)["transaction"]["signatures"][0])
            except Exception:
                pass
    todo = [s["signature"] for s in ok if s["signature"] not in have]
    print(f"to fetch: {len(todo)}")
    if todo:
        with open(tx_path, "a", encoding="utf-8") as f:
            for i in range(0, len(todo), 2000):
                for tx in batch_txs(todo[i:i + 2000]):
                    if tx:
                        f.write(json.dumps(tx) + "\n")
                f.flush()
    print("done")
