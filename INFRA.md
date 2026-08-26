# Running this on bare metal

Written for Latitude.sh, but nothing here is specific to them beyond the region names.

## The thing that decides everything

The internal path is ~26µs from detection to bytes on the wire. The network hop to a
provider is **milliseconds**. So the box's job is not to be fast at computing — that is
already done — it is to be *close* to the providers and to the leaders, and to never drop a
shred.

Ordered by how much they matter:

1. which metro the machine is in
2. not losing shreds to a full socket buffer
3. single-thread clock speed
4. everything else

## Region

Providers publish endpoints in a fixed set of metros, and Solana's validator population is
concentrated in the same places. Pick a Latitude region that *is* one of them rather than
near one.

| Latitude region | provider coverage | notes |
| --- | --- | --- |
| **Frankfurt (FRA)** | jito, 0slot (`de2`), astralane (`fr`), node1, nextblock, nozomi (`fra2`), flashblock, blockrazor | densest EU validator population; `NODE_REGION=fra` already |
| **Amsterdam (AMS)** | jito, 0slot, astralane, node1, nextblock, nozomi, bloxroute, flashblock, blockrazor | as good as Frankfurt, sometimes better peering |
| **New York (NYC)** | jito, 0slot, astralane, node1, nextblock, nozomi (`ewr`), bloxroute, flashblock, blockrazor | best US coverage |
| Ashburn (IAD) | helius sender, nozomi (`ash`) | large validator presence, thinner provider list |
| Tokyo (TYO) | jito, 0slot (`jp`), astralane (`jp`), node1 (`tk`), nextblock, nozomi, bloxroute, flashblock, blockrazor | worth it only if targeting APAC leaders |
| Chicago, Dallas, LA, Sydney, São Paulo | patchy | avoid for this workload |

Frankfurt or Amsterdam if you keep `NODE_REGION=fra`. Do not pick a region because the
machine is cheaper there; a 20ms disadvantage cannot be recovered by any amount of tuning.

**Set the box's resolver to one that supports EDNS Client Subnet** — Google `8.8.8.8` or
OpenDNS `208.67.222.222` / `208.67.220.220`. bloXroute runs each region on several bare-metal
providers and picks the DC from the client subnet the resolver passes through; Cloudflare
`1.1.1.1` strips it, so the region hostname can resolve to a distant POP no matter which
Latitude region the box is in. Same reason to avoid a VPN or a corporate resolver on the
sniper box.

Verify before committing, from a trial box:

```bash
for h in de2.0slot.trade fra.node1.me fr.gateway.astralane.io fra.nextblock.io \
         fra-sender.helius-rpc.com frankfurt.mainnet.block-engine.jito.wtf; do
  printf '%-45s ' "$h"; ping -c 20 -q "$h" 2>/dev/null | tail -1
done
```

Anything in-metro should be well under 2ms. If Frankfurt endpoints answer in 15ms, the box
is not really in Frankfurt.

## Machine

The workload is a handful of latency-critical threads, not a throughput problem:

| threads | what they do | need |
| --- | --- | --- |
| 1–4 `ssListen*` | receive shreds off UDP | kernel-side, needs IRQ locality |
| 1 `shred_reconstructor` | FEC, deshred, detect, patch | **the hot one**, wants an isolated core |
| 6–12 `snipeTx_*` | sign + write per provider | isolated core, spinning |
| a few background | nonce, blockhash, whitelist, positions | anything |
| node process | selling | anything |

So: **16 cores is plenty, and clock speed beats core count.** ed25519 signing (11.3µs here)
and the PDA derivations are pure single-thread work and they sit directly on the path.

- CPU: newest generation available with the highest base/boost clock. A Ryzen-class desktop
  part at 5GHz+ will beat a 32-core server part at 2.4GHz for this specific job.
- RAM: 32–64GB is ample unless you also run a validator (see below).
- Disk: any NVMe. Nothing on the path touches disk.
- NIC: 10Gbps is far more than enough; what matters is the interrupt path, not bandwidth.

**Benchmark the candidate before you commit to it.** The repo measures the two numbers that
matter:

```bash
cd shred-sniper
CARGO_TARGET_DIR=$HOME/t cargo test --release -p sniper bench_ -- --nocapture --test-threads=1
```

Look for `ed25519 sign` and `find_program_address`. This machine does 11.3µs and 2.7µs. A
good box should beat both. If it does not, the CPU is the wrong one.

## The shred source, and a deadline

**Jito ShredStream shuts down on 5 September 2026.** Upstream prints the notice and the
binary refuses to run in `shredstream` mode after that date. Options, in order of effort:

1. **DoubleZero Edge** — what Jito points at. The proxy already has multicast support
   (`--multicast-bind-ip`, `--multicast-device`, default device `doublezero1`), so this is a
   config change rather than a code change.
2. **A provider's shred feed** — several of the providers already configured sell one.
3. **Your own validator** — a non-voting node needs ~256–512GB RAM, 24+ cores and two NVMes,
   which is a different machine class and a much larger bill. Only worth it if you are
   already running one.

Until then, `shredstream` mode with `--desired-regions` set to your metro.

For RPC — nonce refresh, the pump global account, position polling — Helius is fine. None of
it is on the hot path. A local RPC only helps if you were running a validator anyway.

## Host tuning

`scripts/tune.sh` applies the runtime half. Two settings in it matter more than the rest:

- **`net.core.rmem_max=128MB`.** Shreds arrive as UDP. A full socket buffer drops them, a
  dropped shred can cost a whole FEC set, and a lost FEC set is a missed launch. This is the
  single most valuable sysctl here.
- **`net.ipv4.tcp_slow_start_after_idle=0`.** Provider connections idle between launches. By
  default the kernel discards the congestion window after one RTO of idleness and slow-starts
  the next send — precisely the send you care about. The 6s keep-alive pings do not prevent
  this on their own.

The boot half has to go on the kernel command line:

```
isolcpus=6,7 nohz_full=6,7 rcu_nocbs=6,7 processor.max_cstate=1 intel_idle.max_cstate=0
```

Then, after the proxy is up:

```bash
sudo ./scripts/tune.sh --pin
```

which puts `shred_reconstructor` on core 6 and the senders on core 7 under `SCHED_FIFO`, and
keeps the listeners and NIC interrupts on cores 0–3.

C-states deserve a note: waking a core from a deep C-state costs microseconds, the same order
as the entire detect path. `processor.max_cstate=1` handles it at boot; holding
`/dev/cpu_dma_latency` open with a zero written to it does the same at runtime, which is what
a "latency hog" systemd unit is for.

## Spinning versus parking

`SNIPER_SENDER_SPIN_MICROS` decides whether the sender threads spin on their queue or park.
Measured: handing a job to a parked thread costs the detect thread **6.4µs per provider**, to
a spinning one **150ns**.

The default is 2s of spinning after each job, which covers bursts and gives the cores back
when quiet. On a dedicated box, set it high enough that they never park:

```
SNIPER_SENDER_SPIN_MICROS=600000000
```

That burns one core per provider. With 6 providers on a 16-core box that is a fine trade; on
a 4-core box it is not.

## Layout on the box

```
/opt/sniper/
  jito-shredstream-proxy        the binary: shreds, detection, buying
  sniper.json                   generated from .env, contains keys, chmod 600
  keypair.json                  the buying wallet, chmod 600
  whitelist.txt                 launchers to buy from
  node/                         the seller
```

Two systemd units, both `Restart=always`. The proxy is the one that matters; if the seller
dies you stop selling but the gate still lifts correctly, because position state is read from
the chain rather than from the seller.

Run them on the host, not in Docker, unless you have a reason. If you do use Docker:
`--network host` (a NAT hop on the shred socket is not worth it), `--ulimit memlock=-1`, and
`--cap-add=SYS_NICE` so `chrt` works.

Build on the target box, or on an identical one:

```bash
export CARGO_TARGET_DIR=$HOME/sniper-target      # the repo path has a space in it
RUSTFLAGS="-C target-cpu=native" cargo build --release
```

`target-cpu=native` is worth ~5% on signing and nothing elsewhere, but it is free.

## Before going live

```bash
# 1. the transaction is valid against the deployed program
cargo run -p sniper --example simulate -- sniper.json <a mint still on the curve>

# 2. the strategy makes money on paper, without spending any
SNIPER_GHOST_MODE=1 ./jito-shredstream-proxy shredstream ...

# 3. one real round trip, then stop
SNIPER_GHOST_MODE=0 SNIPER_TEST_MODE=1 ./jito-shredstream-proxy shredstream ...

# 4. live, one token at a time
SNIPER_TEST_MODE=0 SNIPER_SYNC_MODE=1 ./jito-shredstream-proxy shredstream ...
```

Do not skip 2 and 3. Ghost mode prices real launches against the real curve and costs
nothing; test mode proves the whole loop with exactly one position at risk.

## What to watch

`snipes_fired` against `pump_creates_seen` tells you whether the whitelist is too tight.
`dropped_full` and `send_errors` per provider tell you whether a provider is wedged.
`skipped_no_nonce` means the nonce pool is too small for the launch rate — add accounts.
`never_landed` climbing means tips or CU price are too low for the current competition.
