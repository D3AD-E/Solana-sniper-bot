# CLAUDE.md

Research notes for the sniper reverse-engineering work in `analysis/`. Read this before
re-deriving anything — most of it cost a lot of tape to establish.

## Prior art

The work started as a replication of this notebook, which reverse-engineered a *different*
sniper wallet:

<https://github.com/avikds/Kaggle-Notebooks-Avik/blob/5279d47e55339a2b00c015742ad1efb794ff0bec/solana-sniper-bot-reverse-engineering.ipynb>

It cannot be run as-is — it depends on a Kaggle dataset that is not available, and its subject
wallet is not either of the two studied here. It is useful only for its framing (feature
construction, the questions it asks). Every number in this file was derived independently from
the replay tape; none of it is carried over from that notebook.

## Data source: pumpapi.io historical replay

**Do not burn Helius credits pulling history.** The full pump.fun event tape is downloadable:

```
https://replay.pumpapi.io/YYYY/MM/DD/HH.jsonl.zst
```

Docs: <https://pumpapi.io/historical-replay>

One file per UTC hour, newline-delimited JSON, zstd-compressed. ~585 MB compressed per hour;
streaming and filtering an hour takes ~21 s. `analysis/extract_replay.py` does exactly that —
it streams the hour, keeps `pool == "pump"` events for tokens created inside that hour, trims to
the first 120 s of each token's life, and writes a parquet tape to `analysis/data/<folder>/`.

Tape columns: `mint, block, ts, action, signer, quote, tokens, vquote, vtokens, price, prio, feerate`.

Days already extracted (see `DAYS` in `analysis/engine.py`):
`2026-08-13 -> replay13`, `2026-08-20 -> replay`, `2026-08-23 -> replay23`,
`2026-08-24 -> replay24` (out-of-sample day, extracted by `extract_replay24.py` into its
own folder — never add it to `data/replay/`, that folder is the 08-20 day).

Helius is still used, but only for what the replay tape does not carry: fee/tip breakdowns and
failed transactions (`fetch_enhanced.py`, `fetch_failed.py`, `e4ez_tips.py`).

### Tape gotchas that will silently ruin results

* **`vquote` on an event is the curve state AFTER that event executes.** To price an order
  landing at event index `i`, use `g[i-1][4]`. Using `g[i][4]` prices you one queue position too
  late, and edge roughly halves per position.
* A buy's `quote` includes the fee, so what reaches the curve is `quote/(1+fee)`. A sell's
  `quote` is net of fee, so what leaves the curve is `quote/(1-fee)`. `fee = 0.0125`.
* Curve invariant: `K = 30 * 1_073_000_000`, `vtokens = K / vquote`. Your own tokens sit
  *outside* the curve — do not subtract them from `vtokens` when pricing an exit.
* The bonding curve **completes** near 85–115 SOL of virtual quote and the token migrates to
  PumpSwap. Walking the constant product past that point is meaningless; cap at
  `COMPLETE_VQ = 115.0`. Not capping produced a fake +12,387 SOL total, 97% of it from ten
  100+ SOL positions.
* When replacing a wallet's trade with your own counterfactual, **skip both its buy and its
  sell** from the forward flow, or you double-count its round trip.
* Never floor flow at zero. `max(mkt_vq - vq - displace*eff, 0.0)` deletes every losing trade
  and yields a fake 95.7% win rate.

## The two wallets studied

Both are pump.fun snipers (not token creators). Measured over the three replay days above, from
real on-chain round trips — no simulation.

### `24678QKx2Dy8ZCw6Ra8o9DeTqPLL5GR9ZQKxt5FddHmq` — "the target"

The original subject. High-frequency, low-margin grinder.

| | |
|---|---|
| round trips | 4,548 over 3 days |
| net | +85.4 SOL (+28.5/day) |
| win rate | 29% |
| net per trade | +0.019 SOL |
| median size | 0.81 SOL |
| median entry rank | 5 |
| median hold | 5 slots |
| creators | 246 (18.5 trades each) |
| fees | ~0.0094 SOL/attempt (prio 0.0025, tip 0.0023 median) |
| tipped | 99.7% of buys |

Uses **durable-nonce pre-signed transactions on 100% of buys**, which forces the token amount to
be precomputed — the pump.fun `buy` instruction (discriminator `66063d1201daebea`) takes
*(exact token amount, max_sol_cost)*, so a round SOL budget is converted to tokens at build
time. Fixed 110,000 CU limit.

Loses on average per trade; the profit is entirely a fat right tail.

### `E4EzXdwf7NNdqM2XGswWaWHfxgucVCo24PTCcrimTKBz` — "the leader"

Found by scanning the tape for the most profitable snipers. Far better operator than the target.

| | |
|---|---|
| round trips | 1,006 over 3 days |
| net | +631.6 SOL (+210/day gross; ~+165/day once his real fees are charged) |
| win rate | 70% |
| net per trade | +0.628 SOL |
| median size | 1.98 SOL |
| median entry rank | 4 |
| median hold | 9 slots |
| creators | 406 (2.48 trades each, 65% seen exactly once) |
| fees | **0.1188 SOL/attempt** (prio median 0.0200 / mean 0.0933; tip median 0.01225 / mean 0.0255) |
| tipped | 96.9% of buys |

Deploys nothing himself. 100% slot-lag-0 — always lands in the create block. One buy per mint.
Budgets are round: 70.8% are exact multiples of 0.10 SOL (1.20 / 1.30 / 1.50 / 1.80 / 2.00 /
3.00 / 4.00), median 2.00.

His fees are **12.6× the target's**. Cutting them is not an option — a lower fee simply means
not landing at rank 4.

**Selection is not creator-based.** Take rate is 1–5% in every bucket of followers, rug
coefficient, migrations, and dev buy; he takes 0.91% of all launches overall. What predicts a
take is **create-block competition**: 0.10% take rate with 0 other buys in the block, rising to
9.16% with 8+. The trigger is a live read of the block being built, so no static DB table can
encode it — it has to be computed off the shred stream.

**Sizing tracks confirmed demand**, not the creator. Tightest ratio of the candidates is
`budget ≈ 0.47 × SOL already confirmed in the block` (IQR 0.31–0.88, CV 1.16 — vs CV 4.4 for
curve depth, 7.8 for dev buy). Median budget by confirming flow: 0 SOL → 2.00; 1–2 → 1.60;
4–8 → 1.98; 8–15 → 3.00; 15+ → 3.47. His ROI rises with size (4.8% → 24%), but that is because
size proxies setup quality — the pooled within-wallet demeaned test is flat, so size scales edge
without creating it.

**Exit is a ladder, and the ladder beats every fixed dump.** 99.5% of positions are laddered:
~30% at slot 1, then ~17% per leg at slots 6, 8, 12, 16. Price-conditional — at ≤0.8× he cuts
55% in one leg; at ≥2× he trims only ~4.5% per leg over 24 slots.

## Settled results (p99-trimmed, real fees charged)

Exit policy, net SOL per trade:

| exit | E4Ez | 24678QK |
|---|---|---|
| dump slot 1 | +0.286 (77.4% win) | −0.017 |
| dump slot 3 | +0.352 (71.5%) | −0.008 |
| dump slot 8 | +0.427 (65.3%) | −0.037 |
| dump slot 30 | +0.532 (54.0%) | −0.127 |
| **their own exit** | **+0.475 (73.4%)** | **+0.008 (32.3%)** |

Both wallets' own exits beat every fixed-slot dump. An earlier "dump at 3 slots is optimal"
conclusion was **wrong** — it came from the uncapped-migration and untrimmed-tail bugs above.

Entry confirmation, measured on real trades at 1.5+ SOL with size held constant:

| buys ahead of you | n | net/trade | win |
|---|---|---|---|
| 0 | 2,308 | +0.528 | 47.0% |
| 1 | 855 | +0.469 | 48.8% |
| **2–3** | **1,292** | **+0.538** | **60.8%** |
| 3–5 | 859 | +0.249 | 53.0% |
| 5–8 | 327 | +0.117 | 47.1% |
| 8+ | 73 | −0.108 | 39.7% |

Other established facts:

* Insiders (wallets touching <20 creators) collectively **lose** 3,562 SOL; generalists (100+
  creators) make +1,858. Copy generalists, not insiders.
* Dev dumps within 3 slots happen on 11% of launches and cause **71% of all losses**. The DB's
  `rug_coefficient` predicts the dump rate across buckets: 0.02% → 8.8% → 15.4% → 34.4% → 59.1%.
* Creator-edge filtering **fails out of sample** (+0.104/trade vs +0.146 taking everything) —
  the worst quartile mean-reverts. The `creator_edge` table exists but should not gate entries.
* A constant-product round trip with no intervening flow returns exactly your SOL at any size
  (measured 0.00%), so "curve impact kills big sizes" is false in isolation.
* Capping at 2 SOL retains 49% of E4Ez's PnL on 55% of his capital — no efficiency loss.
* An ML model trained on simulated entries was **discarded**: on launches the target also took,
  at identical depth (7.94) and rank (5), it claimed 70% win where he actually got 35%. It had
  learned a simulator artifact.

## Postgres

A local database of pump.fun creator metadata already exists — query it before scraping
anything. Connect with:

```
postgresql://pumpinfo:pumpinfo@localhost:5433/pumpinfo
```

Python side uses `psycopg` (v3), e.g. `analysis/e4ez_entry.py`:

```python
import psycopg
DSN = "postgresql://pumpinfo:pumpinfo@localhost:5433/pumpinfo"
with psycopg.connect(DSN) as conn:
    cur = conn.cursor()
    cur.execute("""select wallet_address, followers, rug_coefficient::float,
                          migrations, tokens_created, early_sell_tokens from creators""")
```

Note `rug_coefficient` comes back as `Decimal` — cast it (`::float` in SQL, or
`pd.to_numeric`) before mixing it with float columns, or the arithmetic raises.

Tables:

* `creators` — wallet_address, followers, rug_coefficient, migrations, tokens_created,
  early_sell_tokens. Coverage is partial: only a fraction of tape creators have a row, and take
  rate is statistically flat across every bucket of these columns, so they do **not** explain
  the leader's selection. `rug_coefficient` is still useful — it predicts the dev-dump rate.
* `creator_edge` — written by `analysis/creator_edge.py`. 658 creators; snipes, wallets,
  net_per_snipe, net_total, win, roi_med, roi_p90, size_med, rank_med, hold_med. Indexed
  `creator_edge_net on (net_per_snipe desc)`. Descriptive only — see the out-of-sample warning
  above.

Convention trap: dev-buy values from Helius include the +0.00169 SOL create fee; the DB values
do not. Floors calibrated on one convention silently reject ~88% of matches when applied to the
other.

## Code layout

* `analysis/engine.py` — calibrated pricing engine. Everything else imports `load`, `grouped`,
  `price`, `curve_delta` from here. Reproduces the target's realised PnL at correlation 0.95.
* `analysis/extract_replay.py` — replay download and filter.
* `analysis/snipers.py` — 180,900 real round trips extracted from the tape. Ground truth.
* `analysis/e4ez.py`, `e4ez_entry.py`, `e4ez_tips.py` — leader mechanics.
* `analysis/exit_final.py` — the corrected exit counterfactual (migration-capped, trimmed, fees
  charged). `exit_truth.py` is the earlier uncorrected version; prefer `exit_final.py`.
* `analysis/confirm_rule.py` — create-block confirmation entry rule.
* `rust-native/src/selection.rs` — hot-path selection module for the shred proxy. `arc-swap` is
  the only dependency; 7 tests pass; 1.5–4.9 ns reject, 7.7–8.8 ns accept. Wiring doc in
  `rust-native/SELECTION.md`, config in `creators.tsv`.
  **The napi bindings in `lib.rs` are compile-unverified** — `cargo test` on the full crate fails
  on Windows because solana-sdk needs OpenSSL/MSVC. Verify on a Linux target.
* `analysis/report/sniper-teardown.html` — published artifact. **STALE.** Its strategy section
  still carries simulator-based numbers that later real-trade work contradicted (hold length,
  per-creator sizing, rank). Rewrite before relying on it or sharing it.

## The assembled strategy (frozen 2026-08-25, backtested through OOS)

The selection mystery resolved into three live-computable conditions. Scripts:
`selection_deep.py`, `capacity.py`, `fire_features.py`, `filter_select.py`,
`bt_final.py` (engine), `validate_ladder.py` (certification), `final_eval.py` (frozen
run), `oos_profile.py` (risk). Feature dataset: `data/fire_features.parquet`.

**Why he buys what he buys — what we established:**

* He fires with 0–3 buys ahead of him (median 3.1 SOL confirmed), holds ONE position at a
  time (982/1008 entries with zero open), runs 24/7, entry gaps median 166 s.
* He sends via Jito bundles — 0 failed txs in 3,000; lost races leave no on-chain trace,
  so his true attempt set is unobservable and take-vs-skip comparisons are contaminated.
* No wallet-following: co-sniper clusters exist (lift up to 0.48) but cover <2% of takes.
  Reputable-bot presence ahead is a *negative* signal. Creator DB stats: flat (he ignores).
* What actually predicts a PROFITABLE fire (not "his" fire): **dev freshness + confirmed
  flow**. Fires on a dev's first launch of the day: +0.21/trade; dev with no DB row:
  +0.23; serial launchers (10+ that day): −0.05. `sol_conf >= 12` at fire: +0.32;
  `>= 20`: +1.01/trade at 77% win. Momentum WITHOUT dev freshness loses (−0.06).
  Dev dumps (17% of fires) cost −0.22 vs +0.08 clean; rug_coefficient predicts the dump
  but NOT pnl (high-rug launches also pump harder — net wash, which is why he ignores it).

**Fees:** within-block queue position is NOT bought with priority fee (within-launch
spearman(prio, rank) = −0.12; 52% of rank≤4 buyers in hot blocks pay <0.001 prio). It is
bought with Jito tips + latency. The leader's per-buy joint: 39% tip-only (median total
0.014), 58% tip+heavy-prio (median 0.068); attempt mean 0.119, median 0.035.

**Frozen config** (all parameters chosen on 08-13+08-20 only):

* Entry: watch create block on shreds; fire when `n_conf >= 3` AND `sol_conf >= 4.0` (net
  of 1.25% fee) AND dev's first launch of the UTC day. Land at the very next queue
  position (slip 1).
* Size: `clip(0.47 * sol_conf, 1.5, 3.5)` snapped to 0.1 (median lands at the 3.5 cap).
* Exit: ladder 30%@+1, 20%@+6, 20%@+9, 15%@+13, 15%@+18, force-out @+24; dump all if
  value multiple <= 0.8 at any check; >= 2x trims 5% instead of the leg.
* Costs: attempt 0.035 (tip-only Jito level), 0.001/sell leg, fill 80% with attempt fee
  paid on misses.

**Results (p99-trimmed expected SOL/day):** train 232 / 223, test (08-23) **+263**, OOS
(08-24) **+338** — vs E4Ez's real +103 / +123 those days. Win 50–58%, ~1,300–1,900
fires/day, max 3 concurrent positions, max intraday drawdown 5.2 SOL, all 24 OOS hours
positive, bankroll ~15–20 SOL. Dump-all-at-slot-8 exit scores ~10% higher in-sim but the
ladder is the certified variant (see below). At the leader's full 0.119 mean fee the OOS
day is still +179 trimmed.

**Validity:**

* Exit machinery certified against ground truth: replaying his 993 real entries through
  our ladder gives +0.49/trade vs his real +0.56, corr 0.85, spearman 0.92
  (`validate_ladder.py`) — the sim slightly UNDER-claims at his operating point.
* The sim is fantasy at low ranks: firing at `n_conf=1` claims +3,867 SOL/day; real
  round trips say first-buyer entries make +0.20/trade and 3rd-buyer entries LOSE
  (−0.085, `rank_anchor.py`). Never trust sim results below `n_conf=3`.
* **Everything dies on slip.** slip=1 (land immediately after trigger): profitable.
  slip=2: ~zero. slip=3: negative. This is a latency business; the edge pays for the
  Jito tip and the shred infrastructure, not the other way around.
* Capacity: 1,400 fires/day at rank 4–6 is 6× the leader's volume; assume fee escalation
  and displacement if actually run at that scale. Paper-trade first.

## v1.1: the wallet layer (2026-08-25, triple-checked)

Scripts: `triple_check.py`, `leader_hunt.py`, `walletlist_value.py`, `strategy_v11.py`,
`build_watchlists.py`. All watchlist evaluations use lists built from PRIOR days only.

**Triple-check of v1 passed:** headline reproduces exactly (+262.9 test / +337.9 OOS
trimmed). Dev self-buy wash is negligible (0.7–0.8% of confirmed SOL; those fires
actually win MORE). Size cap 2.0 keeps only ~57% of PnL on 66% of capital — keep 3.5.

**Wallet rotation is structural.** Fires whose confirmers are 100% never-before-seen
wallets are the BEST bucket (+0.30/+0.26 trimmed, win 63–68%, both held-out days).
Sniper wallets rotate constantly, so E4Ez does not follow wallets — he counts flow.
Any broad "reputable sniper" list decays in days and following it LOSES (−0.05..−0.06
per trade). This kills the naive copy-trade idea.

**But three narrow wallet classes carry real, causal signal** (`leader_hunt.py` found
68 wallets profitable on every active day; E4Ez is only #2 by 4-day net):

* **symbionts** (follow): single-dev insiders — ≤3 devs, 40+ trips, >+0.5/trade, net>30.
  E.g. `G3fk9Nyk…` +1,161 SOL net on 218 trades riding ONE creator at rank 2. Bare
  copy-trigger (enter right after their create-block buy, 2.0 SOL, frozen ladder):
  +0.24/+0.21 trimmed, win 76%/69% — +79/day test, +128/day OOS.
* **whales** (follow): size ≥3, >+1.0/trade, 30+ trips. Copy-trigger +0.24/+0.40
  trimmed, +25/+80 per day.
* **elite operators** (avoid): ≥20 devs, 40+ trips, net>30, every day positive.
  If one landed BEFORE our trigger, the fire averages ≈0 or negative — skip it.
  Following them also loses. They are competition, not signal.

**v1.1 book** = MAIN (v1 trigger + dev not in `dev_history` + no elite ahead)
→ else SYM follow → else WHALE follow, one position per launch:

| day | MAIN | SYM | WHALE | book |
|---|---|---|---|---|
| 08-23 test | +261 (n 852, win 66%) | +78 | +7 | **+346** |
| 08-24 OOS | +336 (n 1,356, win 59%) | +128 | +39 | **+503** |

Cross-day dev freshness (amendment): dev seen on ANY prior extracted day is dead
weight (+0.03 vs +0.26/trade test) — 20% of v1 fires removed.

**Postgres tables** (same pumpinfo DSN): `watch_wallets` (75 rows: 33 symbiont-follow,
14 whale-follow, 28 elite-avoid; wallet/tag/action + full stats) and `dev_history`
(39,467 deployers, first/last seen, launches) — powers the live cross-day freshness
check. Rebuild after each new extracted day with `build_watchlists.py` (idempotent;
extend its `SEQ`). The watchlist rots if not refreshed — symbionts die with their
creator, whales rotate within ~a week.

## Live paper-trading rig

`analysis/live_paper.py` runs the full v1.1 decision tree against live pump.fun flow:
Helius WS `logsSubscribe` on the pump program (a create is a tx logging BOTH
"Instruction: Create" AND "InitializeMint2" — "Instruction: Create" alone also matches
ATA creation inside ordinary first-buys and produced garbage candidates), then
`getSignaturesForAddress(mint, until=create_sig)` at create+2.5 s to retroactively
capture the create-slot buys (a WS sub opened after the create MISSES them - don't
"fix" this back to a subscription), decision, second poll at +14 s for the ladder walk.
Uses `dev_history` + `watch_wallets` from Postgres. `N_ENTRIES` env caps entries;
results append-merge into `analysis/data/live_paper.json`.

First live session (2026-08-25 15:46–16:01 UTC): 354 creates, 64% rejected non-fresh,
6 fires. Five strategy-valid trades: −0.074, −0.138, −0.142, +0.007, −0.125 (1/5 win —
14% probability at the backtested 54% rate, n meaningless, and it was the historically
worst hour). The sixth fire was an 85-SOL bundle sweeping its own curve to vq 115 and
insta-rugging (−3.26 in paper): `run_launch`'s `vq >= COMPLETE_VQ` guard was missing
from the live port and is now in. Ladder contained every valid loss (worst −0.14 while
one token went to literal zero — ladder recovered 3.40 of 3.5).

Second session (16:15–16:28 UTC, 10 entries): **+0.56 SOL, 5/10 wins** — win rate on
the backtested 54%, per-trade +0.056 vs expected +0.18 (n=10 variance). Banding
reproduced live: the four fires with ≥12.8 SOL confirmed went 3/4 avg +0.23; the two
at the 4.0–4.3 floor both lost. One Helius WS drop (WinError 121) killed the first
half mid-run — the script now auto-reconnects and persists every close immediately.
Third session (16:30–18:00 UTC, 90 min, 20 entries): **+3.07 SOL, 11/20 wins (55%),
+0.153/trade** — on top of the backtested +0.18 at 54%. 3,799 creates seen, 78%
skipped non-fresh, 782 no-confirmation, 4 bundle sweeps blocked by the vq>=115 guard
(one at vq 988 — likely curve-account misparse on an odd create; the guard turns
those into harmless skips), 2 elite-avoid skips, 1 WS drop auto-recovered.

Running total: 35 valid paper trades, **+3.16 SOL, 17/35 wins (49%)**, worst loss
−0.41, all in `data/live_paper.json`. A tight live-vs-backtest verdict still needs
~300+ entries (`N_ENTRIES`/`TIMEOUT_S` env vars).

## Production wiring (2026-08-25)

* `rust-native/src/strategy.rs` — v1.1 hot path (fresh-dev + confirmation trigger +
  vq>=115 guard + watch wallets + env-driven params). Imports curve math from
  `selection.rs` (which remains the OLD creator-whitelist strategy). 17 tests pass
  standalone in WSL — see `rust-native/STRATEGY.md` for wiring and the test recipe.
  **Run cargo in WSL, not Windows** (prod toolchain; full crate needs Linux OpenSSL).
* All strategy knobs live in `.env` ("v1.1 confirmation-trigger strategy" block).
  `SNIPER_CU_LIMIT=96000` is measured (consumed p50 73k / p99 87k on 149 of the
  leader's landed buys, `analysis/cu_measure.py`) — re-measure on our own landed txs.
* Flat tables for the proxy: `dev_history.txt` (39.5k devs) + `watch_wallets.tsv`
  (75 wallets), regenerated atomically by `analysis/export_tables.py`.
* `analysis/dev_sweep.py` — daemon keeping `dev_history` current from PumpPortal's free
  WS (zero RPC credits), batch-upsert every 30 s, table export every ~5 min, lockfile
  singleton. Installed on this box as scheduled task `pump-dev-sweep` (every 5 min,
  start-if-not-running; `scripts/install_dev_sweep.ps1`). Linux unit:
  `scripts/dev-sweep.service`. If the sweep dies, the freshness filter rots and MAIN
  fires on serial devs — treat a stale `max(updated_at)` in dev_history as a halt signal.
* Deploy: `DEPLOY.md` (Latitude Ubuntu runbook). `scripts/push.sh` rsyncs from dev
  (WSL) with `--build/--restart`; `scripts/bootstrap.sh` (base) then
  `scripts/deploy_v11.sh` (postgres :5433 + tables from `scripts/db/watchlists.sql`
  via `analysis/db_dump.py` + dev-sweep unit). Strategy knobs are .env-only — no
  rebuild to retune; tables hot-reload.
* **`GOLIVE.md`** is the live checklist (what's done, what's left, halt signals) and the
  sizing spec (`size = clamp(0.47×confirmed, 1.5, 3.5)`).
* v1.1 is wired into the REAL prod crate now, not just rust-native: `shred-sniper/sniper/
  src/confirm.rs` (decision machine, ported, own tests), `pumpfun::parse_buy` (create-block
  buy decoder + test), `HotSniper::on_create`/`on_buy` (register-pending then fire),
  forwarder buy dispatch, and the deshred fast path decoding buys only when
  `SNIPER_CONFIRM_MODE=1` (gated by `confirm::CONFIRM_MODE`, so whitelist latency is
  unchanged). `rust-native/src/strategy.rs` remains the standalone/napi twin. Whole
  workspace only compiles on Linux (OpenSSL) — build/test on the box or WSL, never Windows.
  Confirm path is unit-tested but UNPROVEN on live shreds: ghost-soak before trusting it.

## Soak finding: the near-complete-curve trap (2026-08-25)

The 1-hour live paper soak (stalled at 26 min on a WS recv hang — the reconnect guards a
dropped connection, not a silent stall; add a recv watchdog before a long unattended run)
surfaced a real backtest blind spot. Two −3.08 SOL losses at vq≈89 (~59 SOL confirmed) sat
next to a +1.54 win at vq≈87: near-complete curves are bimodal (migrate up, or dump to
zero). The backtest **cannot see this** — it values a near-complete curve AT completion
(`COMPLETE_VQ=115`, optimistic), so `fire_features` shows vq 85–100 as +1.40/trade at 96%
win, which is a simulator artifact of the same family as the discarded ML model. Live and
sim agree only at vq 30–50, where every clean soak fire won (+0.32/+0.46/+0.36). Action
taken: `SNIPER_VQ_CAP_SOL` hardened 115→60 in `.env` (rejects >30 SOL of prior flow, ~3% of
fires, removes the −3 SOL tails). The compiled default stays 115 (the completion constant);
60 is the live-hardened override. Cumulative paper record after this soak: 41 trades,
−3.9 SOL, 20/41 wins — the negative total is entirely the pre-guard −3.26 bundle rug plus
these two near-complete −3.08s, all of which the current guards (vq≥115 sweep guard + the
new 60 cap) now reject.

Clean re-run with the fixes (18:52–19:52 UTC, hardened): the 60s WS staleness watchdog
held the connection for the full unattended hour. 2,928 creates → 595 fresh candidates →
3 entries (2 MAIN + 1 WHALE, +0.169 SOL, all closed). The vq-60 cap rejected 9
near-complete curves; elite-avoid skipped 1; the WHALE-follow book fired live for the first
time. Thin fire count is the slow evening window + a now-comprehensive dev_history (40k
devs, 76% of candidates correctly rejected as repeat deployers) — the freshness filter is
aggressive by design, not broken. The live paper harness (`live_paper.py`) now has the
watchdog and an env-driven `SNIPER_VQ_CAP_SOL` (default 60) matching prod.

## Exit for the v1.1 strategy: dump@8 beats the ladder (2026-08-25)

`analysis/exit_compare.py` holds the frozen v1.1 entry fixed and swaps only the exit, all 4
days, p99-trimmed net SOL/trade:

  dump@8 +0.199 · dump@5 +0.192 · dump@3 +0.185 · **ladder +0.172** · dump@12 +0.169 ·
  dump@1 +0.166 · dump@16 +0.145

The ladder is profitable but ranks 4th — a single dump at slot 8 wins on all four days. The
"their own exit beats every fixed dump" result elsewhere in this file is **E4Ez-specific**
(measured on HIS positions); it does NOT transfer to our fresh-dev + confirmation entries,
which are a different population. Exit-optimal is entry-population-dependent. The headline
backtest and every live-paper session used the ladder, so their PnL is the +0.172 line —
real and profitable, just ~0.03/trade below the dump@8 optimum. **Action for the Node
seller (`src/pump.ts`): keep the single-dump exit (it already does this), just move
`SELL_HOLD_MS` from 1600 (~slot 3-4) toward ~3000 (~slot 8). Do NOT build a ladder — it
underperforms here.** `SNIPER_EXIT_LEGS` in .env is therefore unused by the winning exit;
left in place only for the (worse) ladder variant.

## Exit decision: ladder implemented in the seller (overrides the dump@8 note above)

The dump@8-beats-ladder result (previous section) is true on MEAN but the live A/B
(`live_paper.py` A/B mode: prices ladder/dump8/stop8/dump3 on the same live entries,
`ab_summary.py` tallies) showed the ladder wins on tail-crash tokens — a token fine at slot 3
that dev-dumps before slot 8 is saved by the ladder's unconditional slot-1 30% sell, while
dump@8 and even a reactive stop (which lags a fast crash) eat it. Over the first 7 live
tokens the ladder led. It trades mean for tail risk — E4Ez's reason. **Decision: run the
ladder**, now implemented in `src/pump.ts`: scheduled legs 30/20/20/15/15 @ +1/+6/+9/+13/+18,
force-out @24, per-leg stop (<=0.8x dump all) / moon (>=2x trim 5%), `minSolOutput=0`, partial
legs keep the account open, synchronous `selling` lock prevents double-sells, final leg
retry-hammers. `SELL_LADDER=0` reverts to the single dump. sell tip/priority still static —
dynamic sell tip is the next money-path task. Overnight A/B (`data/ab_overnight.json`) running
to confirm at scale and by size band.

## Money-path audit fixes (2026-08-25)

External audit of template→plan_buy→confirm→sender→nonce→gate→fill→Node ladder→sell. All
findings verified against code before fixing; genuine ones fixed + tested.

* **P0 #1 (DoA):** confirm-mode `on_buy` priced `plan_buy` with the dev buy only, dropping the
  ≥4 SOL of confirmed flow → exact-token request blew past `max_sol_cost` → every confirm
  fire reverted (0% fill). Fix: `ConfirmFire` carries `prior_flow_lamports = vq − VIRT_SOL_0`
  (dev + confirmers, net) and lib.rs prices against it. vq is now all-net (dev buy fee-adjusted
  to match confirmer accounting). Proven by `pumpfun::confirm_flow_must_be_priced_or_the_request
  _exceeds_the_cap` (dev-only pricing exceeds cap; prior-flow pricing fits).
* **P0 #3 (bulletproof backstop):** added orphan reconciliation in `src/pump.ts` — on boot +
  every `RECONCILE_MS` (60s), enumerate the wallet's token accounts (both token programs),
  force-out any untracked bag. Creator read from the bonding-curve account @byte 49
  (`readCreatorBytes`); the seeded account needs no seed to sell. Catches every orphan path
  (#2 dropped fill, #5 late nonce fill, #6 blip, crashes). This is what makes "always sell" true.
* **P0 #2:** `server.rs` fill stream now skips a broadcast `Lagged` instead of ending; Node
  `subscribeToFills` reconnects on `end`/`close`, not just `error` — seller can't go deaf.
* **#6:** `position.rs` distinguishes RPC `Err` from `Ok(None)` — a blip after landing no longer
  fakes a position close (which freed the gate mid-bag in sync mode).
* **#7:** sell retry no longer blind-re-sends the same tokens (would double-sell a landed-but-
  timed-out submit); failed final leg re-enters `runLeg` which re-reads the balance first.
* **#8:** `sendTransactionHeliusSender` parses the JSON-RPC body — a rejected sell (HTTP 200 +
  error) now throws instead of counting as a landed sell.
* **#4:** `SNIPER_SLIPPAGE_BPS` 100→500 (cap is a loss bound, not a spend; 100 reverted when one
  co-sniper landed ahead). Spend still ≈ budget via the haircut.
* **#5:** late durable-nonce fills are covered by reconciliation (#3); advancing the nonce on
  timeout to hard-invalidate is a documented future hardening, not yet built.

Tests: Rust 61 (confirm 8 + tip 5 + pumpfun incl. the #1 regression + the rest), TS 21
(`src/pumpFun/ladder.test.ts`, run with `npm test` / vitest) covering parseLegs, reserves +
creator offsets, constant-product sellValue, valueMultiple, and every `decideLegSize` branch
(normal leg / stop-dump / moon-trim / force-out / clamp / final detection). Ladder math is
extracted to `src/pumpFun/ladder.ts` (pure, testable).

## Money-path audit round 2 (2026-08-26)

Second audit; all verified against code first. Genuine ones fixed + tested.

* **P0 confirming-buy double-count (confirm mode was unsafe):** the early-detect path
  (`deshred.rs` EarlySegment) re-emits every accumulated tx on each new shred, and `on_buy`
  had no dedup → one confirming buy counted N times → trigger fired off a SINGLE buy (exactly
  the rank-1/3 population the backtest says loses). Fix: `parse_buy` carries `sig8` (first 8
  sig bytes); `HotSniper.seen_buys` dedups, cleared on slot advance. NOT obsolete — this is
  the live confirm path; it applies to our strategy.
* **Ghost PnL used the wrong cost** (`buy_lamports`, the unused 1 SOL config, not
  `plan.max_sol_cost`) → ghost-soak per-trade PnL was wrong exactly when you rely on it. Fixed.
* **Dev freshness never expired** → fire rate decayed over uptime, map grew unbounded. Added a
  UTC-day rollover reset in `Session` (matches "first launch of the UTC day").
* **Sell cost basis 5% high:** `entryCostLamports` used `max_sol_cost` = budget×(1+slippage),
  so every value multiple read 5% low and the 0.8× stop fired at ~0.84× real. Fixed: divide
  the slippage back out in `pump.ts` (`SNIPER_SLIPPAGE_BPS`).
* **`.env` seller knobs were dead:** the block used `SNIPER_EXIT_*` names; the Node seller
  reads `SELL_*`. Editing them did nothing. Replaced with live `SELL_*`, defaulted to the
  overnight-A/B winner (stop8: check-legs `1:0,2:0,3:0,5:0,7:0` + force-out `SELL_LAST_SLOT=8`),
  full ladder documented alongside. Added `SNIPER_CONFIRM_MODE=1` (was absent → proxy silently
  ran the empty-whitelist v1.0 path).
* **Gate had no post-landing timeout:** a stuck seller held the sync-mode gate forever, silently
  halting buys. Added `SNIPER_HOLD_CEILING_MS` (120 s) release + `stuck_released` metric; the
  bag is still covered by `reconcileOrphans`.
* **Dynamic tip was provider-blind:** now `max(dyn_tip, provider_min)` so a low Jito-floor
  number can't under-tip non-Jito providers into rejection.
* **Reconcile register-then-abandon:** guarded (skip when `pumpGlobal` unset; drop tracking on a
  force-out that fails to start so the next sweep retries).

Round-2 follow-ups then fixed:
* `sol_lamports` cap-vs-spend: added `SNIPER_CONF_PAD_BPS` (default 0) to discount the padded
  cap toward real spend so the live trigger can be matched to the backtest after a ghost soak.
* Two fee conventions (100 bps chain in plan_buy vs 125 bps tape in on_buy) documented at the
  call site — same lamports, one matches chain, one matches the calibration tape.
* Ladder legs skipped under a slow-RPC selling-lock are now re-queued (250 ms) instead of
  dropped, so the ladder can't silently collapse toward the worst force-out.

Still genuinely un-fixable in code (ops): provision ≥4 nonce accounts to cover the 300 ms
nonce-refresh window — requires creating + funding nonce accounts with the wallet. A fill
dropped while the seller is down still exits at force-out prices via reconciliation (not the
ladder); acceptable and bounded at ≤60 s.

## Standing caveats

The +0.5/trade figures come from a self-selected population of good operators. Copying E4Ez's
parameters does not guarantee reproducing his edge. Paper-trade before risking capital.

The simulator and the real-trade data **disagree on entry timing**. Where they conflict, trust
the real trades.
