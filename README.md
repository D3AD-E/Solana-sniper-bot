# Solana Sniper

Pump.fun sniper that reads launches straight off Jito ShredStream and lands buys in the
create block.

**Execution is under 1 ms.** The shred containing a launch is deshredded, decoded, run
through the entry decision, patched into a pre-built transaction, signed and written to every
provider socket in less than a millisecond. The detect-thread part (`HotSniper::on_create`)
measures 5–7 µs whether it fans out to 1 provider or 12, and signing takes about 10 µs on each
provider's own thread. Latency now comes from the network hop, not the code.

**!!! I AM NOT RESPONSIBLE FOR RISKS AND FUNDS LOSS WHILE USING THIS TOOL !!!**

## How it works

```
.
├── shred-sniper/     rust workspace: the buy path
│   ├── proxy/        modified Jito shredstream proxy (deshredder, gRPC feeds)
│   └── sniper/       detection, entry decision, tx template, provider senders
├── src/              node process: seller (exit ladder), reconciliation, telegram
├── rust-native/      standalone / napi twin of the strategy module
├── analysis/         research: replay-tape backtests, wallet teardown, live paper rig
└── scripts/          deploy, systemd units, provider ping / latency gate
```

- **One process does the buying.** `sniper` is a library compiled into the proxy binary. From
  the UDP shred to bytes queued for the wire there is no IPC, no serialization and no RPC call.
  Nonces, fee recipients, the global config and strategy tables reach the hot path through
  `ArcSwap`.
- **Detection doesn't wait for the segment to finish.** A create is reported as soon as its
  transaction bytes arrive (shred 0 of a 79-shred segment, not shred 78).
- **Pre-built transaction.** Accounts are patched at fixed byte offsets and token-account seeds
  are derived at startup. Durable nonces make the same signed transaction safe to send to every
  provider at once.
- **One body, many providers.** The body is copied once into a pooled buffer. Each provider
  thread stamps on its own tip account, tip and CU price, then signs and sends.
- **Selling happens in Node** (`src/pump.ts`), fed by a `Fill` gRPC stream. Nothing there sits
  on the buy path. Exit is a ladder with a stop and a moon trim. Orphan reconciliation
  force-sells any untracked bag every 60 s.

Full detail: [ARCHITECTURE.md](ARCHITECTURE.md), [shred-sniper/SNIPER.md](shred-sniper/SNIPER.md).

## Providers

Every buy goes to all enabled providers across regions at the same time. Each shares one
durable nonce, so at most one lands:

jito · helius-sender · bloxroute · nozomi (temporal) · blockrazor · falcon · flashblock ·
nextblock · node1 · astralane · 0slot

- Binary / length-prefixed bodies are used wherever a provider supports them.
- Connections are kept warm by per-provider keep-alive probes.
- DNS is resolved once at boot, and reconnects stay off the hot path.
- A boot-time latency gate drops slow endpoints (`SNIPER_MAX_ENDPOINT_MS`).
- Non-2xx submit responses are counted, so a dead key shows up in metrics.

A provider with no key in `.env` is skipped. `<PROVIDER>_REGIONS=none` disables one while
keeping its key.

## Strategy

It doesn't whitelist creators. Instead it watches the create block and fires on confirmed demand:

- Entry: at least N confirming buys and X SOL already in the block, from a dev on their first
  launch of the day. Curves already near completion are skipped.
- Size: scales with the confirmed SOL, clamped to a min/max.
- Exit: laddered sells with a value-multiple stop.

All of these are `.env` knobs, so retuning needs no rebuild. The strategy tables
(`dev_history.txt`, `watch_wallets.tsv`) hot-reload.
[GOLIVE.md](GOLIVE.md) has the sizing and exit spec, and
[rust-native/STRATEGY.md](rust-native/STRATEGY.md) has the wiring.

## Configuration

Everything lives in `.env`. Key groups (see [shred-sniper/SNIPER.md](shred-sniper/SNIPER.md#configuration)
for every knob):

```bash
# wallet / accounts
WALLET_PRIVATE_KEY=          # seller wallet
SNIPER_KEYPAIR=              # buyer keypair path (chmod 600)
SNIPER_NONCE_ACCOUNTS=       # durable nonce accounts, one per concurrent buy
JITO_AUTH_KEYPAIR=           # ShredStream-approved keypair

# rpc
RPC_ENDPOINT=http://127.0.0.1:8899
WEBSOCKET_ENDPOINT=ws://127.0.0.1:8900
RPC_SLOW_ENDPOINT=
RPC_SLOW_WEBSOCKET_ENDPOINT=
COMMITMENT=processed

# providers: <NAME>_KEY + <NAME>_REGIONS (all | comma list | none)
NOZOMI_KEY=  BLOXROUTE_KEY=  BLOCKRAZOR_KEY=  FALCON_KEY=  FLASHBLOCK_KEY=
NEXTBLOCK_KEY=  NODE_ONE_KEY=  ASTRA_KEY=  SLOT_CONNECTION_KEY=
SNIPER_MAX_ENDPOINT_MS=20    # boot latency gate, 0 = keep all
SNIPER_MIN_ENDPOINTS=2

# buy
SNIPER_CONFIRM_MODE=1        # confirmation trigger (0 = legacy whitelist path)
SNIPER_N_CONF=  SNIPER_MIN_CONF_SOL=  SNIPER_VQ_CAP_SOL=
SNIPER_SIZE_RATIO_BPS=  SNIPER_SIZE_LO_SOL=  SNIPER_SIZE_HI_SOL=
SNIPER_BUY_EXACT_SOL_IN=1  SNIPER_SLIPPAGE_BPS=  SNIPER_CU_LIMIT=
SNIPER_DYNAMIC_TIP=1  SNIPER_TIP_MIN_SOL=  SNIPER_TIP_MAX_SOL=

# sell
SELL_LADDER=1  SELL_LADDER_LEGS=  SELL_LAST_SLOT=  SELL_STOP_X=  SELL_MOON_X=
RECONCILE_MS=60000

# safety
SNIPER_GHOST_MODE=1          # decide + log, send nothing
SNIPER_TEST_MODE=1           # one real round trip, then stop

# telegram
BOT_TOKEN=
CHAT_ID=
```

Generate the provider config after any `.env` change:

```bash
cd shred-sniper && cargo run -p sniper --bin gen-config -- --env ../.env --out sniper.json
```

## Build

The Rust workspace only compiles on **Linux** (OpenSSL). Build on the box or in WSL, not
Windows. The repo path has a space in it, so point cargo at a target dir without one:

```bash
export CARGO_TARGET_DIR=$HOME/sniper-target

# rust
cd shred-sniper
RUSTFLAGS="-C target-cpu=native" cargo build --release
cargo test -p sniper

# hot-path timings
cargo test --release -p sniper bench_ -- --nocapture --test-threads=1

# node
npm install
npm run build:napi
npm run build
npm test
```

## Run

| command | what |
| --- | --- |
| `npm run prod` | build and run the seller with production node flags |
| `npm run start` | seller via ts-node |
| `npm run analyze` | analytics |
| `python scripts/ping_providers.py --gate 20` | preview which endpoints survive the latency gate |
| `python scripts/ping_providers.py --reuse-after 8` | check keep-alive holds connections open |

Production runs as systemd units: `sniper-proxy` (shreds + buying), `sniper-seller`,
`sniper-latency`, `dev-sweep`. See [DEPLOY.md](DEPLOY.md) for the bare-metal runbook and
[INFRA.md](INFRA.md) for region, host tuning and DNS.

Before real money, go ghost mode → paper trading → `SNIPER_TEST_MODE=1` (one round trip) →
live, one position at a time.

## License

Apache 2.0
