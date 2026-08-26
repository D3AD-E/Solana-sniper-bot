# In-process pump.fun sniper

The proxy no longer just forwards shreds: it detects pump.fun launches inside the deshred
thread and fires the buy from the same process. There is no cross-process hop, no base64, no
gRPC and no Node.js on the detect → send path.

```
udp shreds ─► listener thread ─► reconstruct thread ───────────────► sender thread(s)
                                   deshred + entries                   sign, base64, write
                                   parse_create                        warm keep-alive socket
                                   whitelist + dedup
                                   patch prebuilt tx  ── crossbeam ──►
                                          │
                                          ├─► gRPC PumpCreate feed  (analytics)
                                          └─► gRPC Fill feed        (node: sells)
```

Node keeps sells, telegram and analytics. It learns what was bought from the `Fill` stream
and exits over a normal websocket connection — nothing on the sell side is latency critical.

## The buy transaction

The layout mirrors a known-good mainnet sniper transaction rather than what an SDK would
emit:

```
0 System        advanceNonceAccount        durable nonce
1 System        transfer                   provider tip
2 ComputeBudget setComputeUnitLimit        90,000
3 ComputeBudget setComputeUnitPrice        per provider
4 System        createAccountWithSeed      token account, 165 bytes
5 Token-2022    initializeAccount3
6 pump.fun      buy                        18 accounts, 25 byte data
```

Two choices carry most of the win:

**Seeded token account instead of an ATA.** The associated-token-account program costs ~18k
compute units and two extra CPI hops. `createAccountWithSeed` + `initializeAccount3` costs
~2k. Measured: 68k CU for the whole buy versus 89k for the ATA version. The seed is six ASCII
digits from a rotating counter, so each buy gets a fresh account, and the sell closes it to
reclaim the rent. The seed is published on the `Fill` stream because the address cannot be
re-derived from the mint alone.

**Durable nonce instead of a recent blockhash.** The transaction never expires, so nothing
depends on a blockhash arriving in time, and the nonce is fetched from a pool that a
background thread keeps warm. Every provider variant of one launch deliberately shares a
nonce: whichever lands first advances it and the rest become invalid, which makes a
double-buy impossible without any coordination between senders.

The `buy` account list is the 16 accounts from the on-chain IDL plus two remaining accounts
the deployed program requires: `bonding_curve_v2` (PDA `["bonding-curve-v2", mint]`) and a
buyback fee recipient. Sending only the IDL accounts fails with `BuybackFeeRecipientMissing`
(0x17ae), and adding just the buyback recipient fails with `InvalidBondingCurveV2` (0x17ba).

### Regenerating the byte-patch offsets

Offsets are never hand-written. `template::build` compiles the instruction list once with
distinctive placeholder values, then locates each placeholder in the serialized message. Edit
the instruction list and the offsets regenerate themselves. To see the result:

```bash
cargo run -p sniper --example dump_offsets
cargo run -p sniper --example dump_offsets -- 120000   # different CU limit
```

It prints every offset, the instruction list and the compiled account keys. If a placeholder
ever appears twice — as the token program does, once as an account key and once inside the
create-account data — the build fails loudly instead of patching only one copy.

## Sizing and slippage

`buy_lamports` is the **total** spend per launch, fees included, and defaults to 1 SOL.

pump.fun `buy` takes an exact token amount plus a cap, so slippage is the error between what
we ask for and what the curve charges. `plan_buy` removes two sources of that error: it
divides the protocol + creator fee out of the budget before pricing, and it prices the curve
*after* the dev buy in the same transaction. `haircut_bps` (default 30) is shaved off the
token amount so a curve that moved slightly further still fits; `slippage_bps` (default 100)
is headroom on the cap only and is never spent unless the curve actually moved.

A unit test inverts the curve and asserts the real cost lands within 1% of the budget and
always under the cap, for dev buys of 0, 0.5 and 2 SOL.

## Nonce accounts

Required — the template advances one. Create a few (one per concurrent in-flight launch is
plenty; four is a sensible start):

```bash
solana-keygen new --no-bip39-passphrase -o nonce1.json
solana create-nonce-account nonce1.json 0.0015 --nonce-authority <buyer pubkey>
solana nonce-account <nonce1 pubkey>          # confirm
```

The authority must be the buying wallet, and startup fails loudly if it is not.

## Providers

Every provider is the same thing: a warm keep-alive connection, a tip account and a compute
unit price. Jito is not special — it uses the same prebuilt template and byte patching as the
rest.

**Keep-alive is not optional and the interval is not arbitrary.** Each endpoint is held open
by a periodic GET to the provider's `health_path`; a provider with an empty `health_path` is
never probed, so its connection goes cold and every launch pays a TCP handshake on the hot
path. The interval (`KEEPALIVE_SECS` in `sender.rs`) is set by the *tightest* provider window,
measured rather than assumed: **helius-sender hangs up after 10 s**, flashblock after 30 s,
nozomi after 65 s. It is 6 s. A non-200 reply is fine — jito and nextblock 404 every path, and
a 404 on an open connection does the job. Verify with:

```bash
python scripts/ping_providers.py                   # every endpoint reachable
python scripts/ping_providers.py --reuse-after 8   # connections survive the probe interval
```

| Provider | Auth | Min tip | Notes |
| --- | --- | --- | --- |
| jito | none | 1,000 | `/api/v1/transactions`, HTTPS, all regions. No health endpoint - `/` 404s, which still keeps the socket warm |
| helius sender | none | 1,000,000 | `/fast`, `GET /ping`. **Closes idle connections at 10s** - tightest window here |
| 0slot | `?api-key=` | 1,000,000 | ny/de/ams/jp/la |
| astralane | `?api-key=` | 10,000 | `/iris` |
| node1 | `api-key` header | — | ny/ams/fra only, no Tokyo or LA |
| nextblock | `Authorization` header | — | `/api/v2/submit`, wrapped body |
| nozomi (temporal) | `?c=` | 1,000,000 | 9 direct http regions, `GET /ping` keep-alive (idle >65s is closed) |
| bloxroute | `Authorization` header | 1,000,000 | wrapped body, `GET /health` keep-alive, 5 regions + `global` edge, needs an ECS resolver |
| flashblock | `Authorization` header | 100,000 | `/api/v2/submit-batch`, `GET /` keep-alive, closes idle at 30s, 7 nodes |
| circular FAST | `x-api-key` header | 1,000,000 | `fast.circular.fi` |
| blockrazor | `?auth=` + `apikey` header | 100,000 | **binary** body, plain HTTP on :443, 11 endpoints, `/health` keep-alive |

jito and helius sender need no API key, so they are the two enabled by default in
`sniper.example.json`. Listing the same host twice with different `cu_price` values sends
both variants. `tip_accounts` is a list and rotates per launch.

Body format is `json_rpc` (default), `wrapped` for nextblock style
`{"transaction":{"content":"..."}}`, `wrapped_blox` for bloxroute (the same wrapper plus
`"submitProtection":"SP_LOW"` — their default, `SP_MEDIUM`, *holds* a transaction until four
consecutive slots are clear of a leader they score as high-risk, which is fatal at slip 2),
`plain_tx`, `batch`, or `binary`.

`binary` is blockrazor's `/v2/sendBinaryTransaction`: the signed transaction goes on the wire
as raw bytes under `application/octet-stream`, with no base64 and no JSON envelope. That is
~26% fewer bytes than the JSON form and removes the encode from the hot path. Verified by
posting one identically-serialised transaction both ways and getting the same downstream
error from the provider.

## Configuration

Copy `sniper.example.json` to `sniper.json`, fill in keys, then:

```bash
jito-shredstream-proxy shredstream \
  --block-engine-url https://ny.mainnet.block-engine.jito.wtf \
  --auth-keypair auth.json \
  --desired-regions ny \
  --sniper-config /etc/sniper/sniper.json \
  --grpc-service-port 9999          # feed for the node seller
```

| Field | Meaning |
| --- | --- |
| `keypair_path` | solana-cli keypair json for the buying wallet |
| `nonce_accounts` | durable nonce accounts, one used per launch |
| `whitelist_path` | one base58 launcher pubkey per line, reloaded when the file changes |
| `whitelist_disabled` | accept every launcher (shadow runs only) |
| `rpc_url` | local validator/RPC for nonces and the pump global account |
| `buy_lamports` | total spend per launch including fees (default 1 SOL) |
| `max_dev_buy_lamports` | skip launches whose dev buy is at least this large |
| `haircut_bps` / `slippage_bps` | see sizing above |
| `cu_limit` | compute unit limit (default 90,000; the buy burns ~68k) |
| `dry_run` | build, patch and sign, but never touch a socket |

## Testing

```bash
cargo test -p sniper                       # template layout, curve math, PDAs, config
cargo test -p sniper --test mainnet_creates # detection against real create_v2 transactions
cargo test --release -p jito-shredstream-proxy  # deshredder + pump-create shred replay
cargo run -p sniper --example dump_offsets      # byte patch table
cargo run -p sniper --example simulate -- sniper.json <mint on the curve>
```

`simulate` is the one that matters before going live: it builds, patches, signs and runs
`simulateTransaction` with `sig_verify` on and the real nonce. A clean simulation means the
account list, instruction data, nonce and compute budget all match the deployed program.

## Deployment

Run the proxy and the node seller in one container, or two sharing `/dev/shm`, with
`--network host` so the shred socket is not NATed.

```bash
# isolate cores at boot: isolcpus=6,7 nohz_full=6,7 rcu_nocbs=6,7
cpupower frequency-set -g performance

pid=$(pgrep -f jito-shredstream-proxy)
for t in $(ls /proc/$pid/task); do
  case "$(cat /proc/$pid/task/$t/comm)" in
    shred_reconstructor) taskset -pc 6 $t; chrt -f -p 80 $t ;;
    snipeTx_*)           taskset -pc 7 $t; chrt -f -p 70 $t ;;
  esac
done
```

`shred_reconstructor` is the detect thread; `snipeTx_<provider>` are the senders. Keep them on
different cores so a blocking socket write cannot delay deshredding.

Later: leader-aware routing — read the leader schedule from the local validator and prefer the
provider/region closest to the current and next leader's TPU instead of firing at everyone.

## Metrics

Counters only, reported off the hot path:

* `shredstream_proxy-service_metrics`: `pump_creates_seen`, `pump_creates_emitted`,
  `snipes_fired`, `recovered_count`, `entry_count`, `txn_count`,
  `fec_recovery_error_count`, `bincode_deserialize_error_count`.
* `sniper::SniperMetrics`: `creates_seen`, `creates_whitelisted`, `duplicates`,
  `skipped_dev_buy`, `skipped_no_nonce`, `fired`, `jobs_queued`.
* per provider: `sent`, `send_errors`, `reconnects`, `dropped_full`.
