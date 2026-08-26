# Go-live checklist

Status of the v1.1 confirmation-trigger sniper on the road to real money. Ordered by what
blocks what. See `DEPLOY.md` for the Latitude box steps, `rust-native/STRATEGY.md` and
`shred-sniper/sniper/src/confirm.rs` for the strategy, `CLAUDE.md` for the research.

## How much we buy each time

The position size is **not fixed** — it scales with the SOL confirmed in the create block,
exactly as the leader sizes. All from `.env`, no rebuild to change:

```
size = clamp(0.47 × confirmed_SOL, 1.5, 3.5) SOL, snapped down to 0.1
```

| confirmed in create block | our buy |
|---|---|
| 4 SOL (trigger minimum) | 1.5 SOL (floor) |
| ~6 SOL | 2.7 SOL |
| ~7.5 SOL and up | 3.5 SOL (cap) |

- **`SNIPER_SIZE_RATIO_BPS=4700`** — the 0.47 multiple. Backtested tightest fit to the
  leader's own sizing; his ROI rises with size because size proxies setup quality, so
  tracking confirmed flow is what matters, not the exact number.
- **`SNIPER_SIZE_LO_SOL=1.5` / `SNIPER_SIZE_HI_SOL=3.5`** — the clamp. Capping at ~3.5
  keeps ~all of the edge on a fraction of the capital (backtest: cap 2.0 held 49% of PnL
  on 55% of capital — no efficiency loss). The median live fire lands at the 3.5 cap.
- **`SNIPER_FOLLOW_SIZE_SOL=2.0`** — flat size when following a symbiont/whale watch
  wallet (a single confirming buy from them is the trigger, so there is no flow to scale).
- **`max_sol_cost`** on the buy instruction = the sized budget. An adverse curve move past
  it reverts on-chain (error 6002) and costs only the tip — this is the real-money slippage
  and bundle-sweep guard, stronger than the paper sim.

**Start smaller, raise after a clean day.** For the first live day set
`SNIPER_SIZE_HI_SOL=2.0`; the strategy is unchanged, the tail is just capped lower.
Bankroll for full size: ~20 SOL (max 3 concurrent × 3.5 + fee float), fund ~25.

## Exit: laddered (implemented in the Node seller)

On mean, dump@8 slightly beats the ladder in backtest (+0.199 vs +0.172). But the live A/B
(`live_paper.py` A/B mode) showed the ladder wins on the **tail-crash tokens**: when a token
is fine at slot 3 then dev-dumps before slot 8, the ladder's *unconditional* 30%-at-slot-1
sale saves the position while a fixed dump@8 (and even a reactive stop, which lags a fast
crash) eats the full loss. Over the first 7 live tokens the ladder led. The ladder trades a
little mean for much lower tail risk — E4Ez's reason for using it. **Decision: run the
ladder.**

Implemented in `src/pump.ts` (typechecks clean):
- Scheduled legs 30/20/20/15/15 at slots +1/+6/+9/+13/+18, force-out @+24
  (`SELL_LADDER_LEGS`, `SELL_LAST_SLOT`, `SELL_SLOT_MS=400`).
- Per-leg price override: `<= SELL_STOP_X` (0.8) dumps all; `>= SELL_MOON_X` (2.0) trims
  `SELL_MOON_FRAC` (0.05) and keeps riding.
- `minSolOutput=0` (never reverts — always exits), partial legs don't close the account,
  final leg closes. Per-position `selling` lock acquired synchronously so two legs can never
  double-sell. Final leg retry-hammers so we never get stuck holding.
- **Escape hatch `SELL_LADDER=0`** → old single dump at `SELL_HOLD_MS`, instant fallback.

Still static and worth making dynamic next: `SELL_TIP_LAMPORTS` / `SELL_CU_PRICE` — laddered
legs each compete to land, so a competitive dynamic sell tip (extend `tip.rs`'s logic) is the
follow-up that keeps legs landing first try. Confirm the ladder at scale with the overnight
A/B before trusting it live.

## 1. Code — DONE, pending full-crate compile on Linux

- [x] Buy decoder `pumpfun::parse_buy` (create-block buys) + test.
- [x] Confirmation state machine `confirm.rs` (fresh-dev, trigger, vq≥115 guard, watch
      wallets, flow sizing) — unit-tested.
- [x] `HotSniper::on_create` registers pending in confirm mode; `on_buy` advances + fires;
      slot sealing; `FiredLaunch` carries seller fields for the buy path.
- [x] Forwarder dispatches buys; deshred fast path decodes create-block buys **only** when
      `SNIPER_CONFIRM_MODE=1` (whitelist-path latency unchanged).
- [x] CU limit corrected 90k→96k from measured burn (p99 87k).
- [x] **Whole workspace compiles + tests green under the Linux toolchain** (WSL on the dev
      box): sniper 54 tests + confirm 7/7, `cargo check --workspace` clean, 0 warnings.
      (`scripts/wsl_build_check.sh`.)
- [ ] Build the release binary on the actual Latitude box (`push.sh --build`) and run its
      self-test — the box has the real CPU flags and OpenSSL.
- [ ] **Ghost-mode soak on live shreds** (`SNIPER_GHOST_MODE=1`, `SNIPER_CONFIRM_MODE=1`):
      confirm fire rate and per-trade match `analysis/data/live_paper.json`. The confirm
      path has unit tests and a WS paper-proxy behind it, but has never run on real shreds.

## 2. Provider access — keys still needed

- [ ] **Jito ShredStream auth keypair** — THE blocker. No create-block visibility, no
      slip-1, no strategy without it. Register/approve now (longest lead time).
- [x] `NEXTBLOCK_KEY` set in `.env` and **verified working** — auth passes on
      `/api/v2/submit` (reaches tx validation), no auth -> 401. 8 endpoints enabled.
- [ ] ...but it is still a **trial** key (the value literally starts `trial…`). It works
      today; nothing warns you when it lapses, and a dead key looks like "nextblock never
      wins a race". Renew or replace before real size.
- [x] `NOZOMI_KEY` set in `.env`, all 9 direct regions enabled and verified (`/ping` 200 on
      every host, bogus key -> 401). Regenerate `sniper.json` after any `.env` edit.
- [x] `BLOXROUTE_KEY` set in `.env`, 6 endpoints (ny/germany/amsterdam/uk/tokyo + `global`
      edge) verified against `/api/v2/rate-limit` and `/api/v2/submit`; bogus header -> 401.
      Rate limit on this account is 3,000 per 60 s — the whole fan-out is 1 request per
      launch per region, so ~500 launches/min before it binds.
- [ ] **Set an ECS-capable DNS resolver on the box** (`8.8.8.8`, or OpenDNS
      `208.67.222.222`) — *not* Cloudflare `1.1.1.1`. bloXroute routes each region's
      hostname to the nearest bare-metal DC using EDNS Client Subnet; a resolver that hides
      the client ASN silently lands us on a far POP. Nothing in `bootstrap.sh` sets this yet.
- [x] `BLOCKRAZOR_KEY` set in `.env`, all 11 endpoints verified (`/health` 200 on every host,
      no auth -> 403). Uses **binary** submission (`/v2/sendBinaryTransaction`, raw bytes, no
      base64): proven by posting one identically-serialised tx as binary and as JSON and
      getting the same downstream error. Min tip is 0.0001 SOL, the lowest of any provider.
- [x] `FLASHBLOCK_KEY` set in `.env`, all 7 nodes verified (`GET /` 200 on every host, no
      auth -> 403). Min tip 0.0001 SOL. lucum and lunar lander (hellomoon) were dropped from
      the catalogue entirely — re-adding one means restoring its `ProviderSpec`, not a key.
- [x] `FALCON_KEY` (Corvus Labs) set in `.env`, all 9 regions verified (`GET /health` 200 on
      every host; submit with no key -> 401, with key -> reaches tip validation). Submits via
      **`/binary`** — raw transaction bytes, same `BodyFormat::Binary` as blockrazor. Min tip
      0.001 SOL. Their native UDP :9000 is faster still but is NOT wired: it needs a
      non-stream transport and never replies, so it cannot be validated without a funded
      live fire.
- [x] **Keep-alive interval corrected.** `helius-sender` closes idle connections after **10
      seconds** (measured); the probe was at 50 s, so every helius send was reconnecting on
      the hot path. `KEEPALIVE_SECS` is now 6 and the probe no longer blocks on replies.
      Re-check any time with `python scripts/ping_providers.py --reuse-after 8`.

## 3. Wallet & accounts

- [ ] Wallet keypair at `SNIPER_KEYPAIR` (`/etc/sniper/keypair.json`, chmod 600), funded
      ~25 SOL.
- [ ] **Rotate `WALLET_PRIVATE_KEY`** before real size — it has sat in `.env` on the dev
      box (git-ignored and never committed, verified, but rotate anyway).
- [ ] 4 nonce accounts in `SNIPER_NONCE_ACCOUNTS` (one per concurrent buy).

## 4. Box (Latitude, Ubuntu, FRA)

- [ ] `push.sh user@box` → `sudo bootstrap.sh --tune` → `sudo deploy_v11.sh` →
      `gen-config`.
- [ ] Boot args `isolcpus=… nohz_full=… rcu_nocbs=… processor.max_cstate=1`, reboot,
      `tune.sh --pin`.
- [ ] `dev-sweep` service green; `dev_history.max(updated_at)` fresh.

## 5. Region / leader routing — decided, no work

- Detection colo in FRA buys slip-1. Submission fan-out is already solved: every provider
  fires its `*_REGIONS` endpoints, all variants share one durable nonce → one lands, one
  tip paid. Keep `all` or trim to `fra,ams` after a week of landed-rank-per-region data.
- Leader tracking is **not needed** on the Jito path (block engine relays to the current
  leader). Only worth building if a direct-TPU QUIC path is added later.

## 6. Prove-it sequence (before and during real money)

1. `SNIPER_GHOST_MODE=1 SNIPER_CONFIRM_MODE=1` — soak ≥1 day, compare to paper rates.
2. `SNIPER_TEST_MODE=1` — one real round trip, inspect it on chain.
3. Live at `SNIPER_SIZE_HI_SOL=2.0`, `SNIPER_SYNC_MODE=1` (one position at a time).
4. Full `3.5` cap after a clean day.

**Halt signals:** `dev_history.max(updated_at)` older than 10 min (freshness filter
rotting → MAIN fires on serial devs); daily loss beyond ~15 SOL (≈3× backtest worst
drawdown); confirm fire rate diverging hard from ghost soak.

## Known limitation of the confirm path

To see create-block buys the proxy must decode far more of each segment than the
create-only fast path did — in confirm mode nearly every segment is walked, not one in six
hundred. That is the real cost of the trigger and why this needs a strong box and a ghost
soak to confirm the detect budget holds under load. Measured decode/latency on the box is a
go/no-go gate, not an afterthought.
