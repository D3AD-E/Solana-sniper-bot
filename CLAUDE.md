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

## The wallets studied

The first two are pump.fun snipers (not token creators). Measured over the three replay days
above, from real on-chain round trips — no simulation. A third wallet, "the drafter", was added
2026-08-26 — a momentum buyer, not a sniper (see below).

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

### `57stAMFvwctAjkBS76RXGoK4QKyS1QoxbGMbzFFe4DyZ` — "the drafter"

Investigated 2026-08-26 on suspicion of being E4Ez under a second key. **He is not** — different
strategy class entirely: a *momentum buyer* who enters well after launch, drafting behind the
create-block fight instead of joining it. Full dossier: `analysis/W57ST.md`; script:
`analysis/w57st.py`; positions: `analysis/data/w57st_positions.parquet`.

| | (4 tape days: 08-13/20/23/24) |
|---|---|
| positions | 754 (188/day), 39 rebuys |
| net (raw, realised) | +960.7 SOL (+240/day) — tips unknown, prio med 0.0043 |
| win rate | 66% closed positions |
| ROI per position | med +8.0%, p10 −16.4%, p90 +50.8% |
| median entry | **46 blocks after create** (p10 7, p90 170), buy rank 46, vquote 47.3 |
| create-block entries | **0 of 754** |
| median size | 1.98 SOL (same 2-SOL convention as E4Ez) |
| exit | ladder, med 5 legs; first sell 1 block after entry, last at block 18 |
| max concurrent | 5 |
| overlap | E4Ez 8% of his mints (E4Ez always first), target 14% |

**He runs two books** (deep pass 2026-08-26, `analysis/w57st_deep.py`): a ~2-SOL bonding-curve
momentum book (743 pos, +331 SOL, 65% win) and an ultra-selective **post-migration whale book**
— 11 positions of 167–261 SOL on runaway AMM pools (med 624 SOL inflow/10 blocks pre-entry),
+1,056 SOL, 91% win, **76% of his net**. Blind migration-sniping loses (−8.4% med); his whale
edge is selection, taking 0.23% of migrations. Bankroll implied: high hundreds of SOL.

Selection is real, not luck: blind curve baseline (vquote≥45, offset≥7, exit +18 blocks) loses
−11% med / 36% win vs his +8% / 66%. Trigger: sustained inflow (take rate 0.5% below 2 SOL/10
blocks → ~4.5% above 5), broad participation (0.16% take at ≤5 unique buyers → 5.7% at 20–40),
and dev-already-sold (1.2% vs 4.9%; hot+dev-sold 6.1%). **Unlike E4Ez, creator features are NOT
flat for him**: take rate doubles (8.4%) at rug_coefficient 0.8–1.0 — he *prefers* guaranteed
dev-dumpers and buys the survivors. Buys into strength: +16% med price slope in prior 5 blocks,
44% within 5% of the local high. Sizing flat (no demand-tracking; 100% round 0.1 multiples);
the whale/curve split is the sizing decision. Exit: 20% at block +1, then ~13.5% per leg every
4–5 blocks; ≤0.8× cuts 30%/leg, ≥2× trims 8%/leg to block ~30.

**Fees near zero** (156-tx on-chain sample): 81% of buys untipped, prio ~0.004 or nothing;
sells 0.00036 flat; a sampled 152-SOL whale buy cost 0.001 total. ~0.004 median per landed buy
vs the leader's 0.035/0.119 — but 41% of his txs fail (spray + slippage guards). Wallet is old:
≥200k sigs, active before 2026-06-04 (E4Ez's wallet: born 2026-08-04) — kills "same guy" again.
Not in `watch_wallets`, `creators`, or `creator_edge`.

Why he matters: E4Ez-level net with **no latency race and ~30× lower fees**. Signals (10-block
inflow, unique buyers, dev-sold, rug_coefficient) all computable from our stream at an
18-second budget; `dev_history` plugs straight in. **The whale book is out of reach at our
15–20 SOL bankroll — ignore it.** The actionable piece is the curve book alone: 2-SOL entries,
max 5 concurrent (~10–12 SOL working capital), +83/day at 65% win over the 4 tape days.
Candidate for a v1.2 side-strategy study; nothing adopted yet. Full plan sketch at the end of
`analysis/W57ST.md`.

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

Tests: Rust 74 (confirm 8 + tip 5 + pumpfun incl. the #1 regression + the rest), TS 21
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

## Sender providers

Catalogue is `shred-sniper/sniper/src/providers.rs` (data only); `gen-config` turns it plus
`.env` into `sniper.json`. A provider with no key in `.env` is silently omitted — check the
"not enabled" list gen-config prints.

### Two keys are DEAD (found 2026-08-26) — 0slot now disabled, astralane still failing

`SLOT_CONNECTION_KEY` and `ASTRA_KEY` do not authenticate. Both providers were in the fan-out
contributing exactly nothing, and **nothing anywhere reported it** — that is the failure the
response-status work below exists to catch.

* **0slot** answers every submit `{"error":{"code":403,"message":"api-key does not exist"}}`,
  byte-identical to what a made-up key returns. Its `/health` still returns `OK`, so
  `ping_providers.py` called all six endpoints healthy. **Now disabled** via
  `SLOT_REGIONS=none`, which drops the provider while keeping the key in `.env`. All the
  endpoint work is already done and waiting — `/txb` binary submission, keyless `/health`
  keep-alive, `ny2`+`de2` direct hosts, 21 tip accounts — so re-enabling is one word once a
  working key exists.
* **astralane** was dead on two successive keys and is **now fixed** with a third
  (2026-08-26). A working key is unambiguous, which is what makes 401 diagnostic here:

  ```
  POST /iris?api-key=<good>   -> {"jsonrpc":"2.0","id":1,"result":"Ok"}          [200]
  POST /irisb  …&method=sendTransaction, garbage bytes
                             -> {"error":"failed to parse binary transaction"}  [400]
  POST /iris?api-key=nope    ->                                                 [401]
  ```

  The 400 is the one that matters: it proves the `/irisb` binary route parsed our request and
  got as far as the transaction. Before concluding a key is dead, sweep the auth styles —
  query `api-key`/`apiKey`/`api_key`/`key`/`token`/`auth` and headers
  `api-key`/`apiKey`/`api_key`/`x-api-key`/`Authorization`/`X-Astralane-Key` all returning
  401 means the credential, not the wiring.

Everything else authenticates: node1, nextblock, nozomi, bloxroute, flashblock, blockrazor,
falcon (jito and helius-sender take no key).

`<PROVIDER>_REGIONS=none` (or `off`) is the switch for this situation. An **empty** value does
not work — `get_or` treats empty as unset and falls back to `all`, so `SLOT_REGIONS=` would
silently mean "every endpoint".

### The response path: `sent` never meant "accepted"

`metrics.sent` counts bytes handed to a socket. Every rejection after that — a rotated key, a
rate limit, a tip under the floor, a body format a provider stopped taking — used to arrive as
an ordinary HTTP response on a healthy connection and be discarded unread by `drain_ready`.
The sniper could reject its entire fan-out and report a clean run, which is how the two dead
keys above survived.

`note_status` now reads status lines off the drain and counts non-2xx submits into
`metrics.rejected`, warning on the first and then every 32nd consecutive failure. Two details
that are not optional:

* **Keep-alive replies must not count.** jito 404s every path and nextblock's `/health` 401s
  for our key, both by design. The probe reply is a round trip away, so it is normally read by
  the *next* drain — usually a launch drain — and a plain counter mis-attributes it to a
  submit. `Endpoint::awaiting` is a FIFO of what each outstanding reply belongs to; HTTP/1.1
  keep-alive guarantees replies come back in request order.
* **Two providers can't be read this way.** flashblock returns HTTP 200 for its own errors
  (`{"code":403,"message":"PermissionDenied","success":false}`) so its rejections stay
  invisible; node1 returns `605` on a good key with a bad transaction, so its counter moves for
  transaction faults too.

**nozomi (temporal)** is keyed and live. It now submits on **`/api/sendBatch`**, the route
their own docs rank first: "Use Batch Send over a direct `http://` endpoint … Plain HTTP
avoids per-transaction TLS encryption; batch avoids JSON and per-request overhead", and it is
"the fastest option even for a single transaction". The three routes, slowest first:

| route | body | size for our ~1,043 B tx |
|---|---|---|
| `POST /` (JSON-RPC) | base64 + envelope | ~1,450 B ← what we used to send |
| `POST /api/sendTransaction2` | base64, `text/plain` | ~1,390 B |
| **`POST /api/sendBatch`** | `[u16 BE len][tx]`, octet-stream | **1,045 B** |

`BodyFormat::LenPrefixedBinary`. Limits are 16 transactions, 66..=1232 bytes each, 19,744 byte
body; we send one. The reply is an empty 200 carrying no signature, which costs nothing — we
already know the signature. Their QUIC client is explicitly *not* faster here: it exists for
"workloads that cannot hold a single connection open", and we hold one.

`NOZOMI_KEY` in `.env`, `NOZOMI_REGIONS=all` → all
9 direct regions (`ewr1 fra2 ams1 lon1 lax1 tyo1 sgp1 pit1 ash1`, plain http on :80; the
digit-less `ewr./fra./…` aliases are Cloudflare, https-only and slower — do not use them).
Auth is the `?c=<key>` query parameter; a bad key returns HTTP 401 `Unauthorized`, a good one
returns a JSON-RPC error for a bad payload. Nozomi **closes any connection idle >65s**, so its
spec carries `health_path: "/ping"` — the sender only keep-alives endpoints that declare one,
and an empty value silently costs a fresh TCP handshake on every launch. Probe cadence is
`sender_spin_micros` (2 s) + the 50 s `recv_timeout` ≈ 52 s, inside the 65 s window.
Regression-guarded by `providers::tests::nozomi_declares_a_keepalive_path`.

**bloxroute** is keyed and live: `BLOXROUTE_KEY` in `.env` is the raw base64 `accountID:secret`
sent as the `Authorization` header (no `Bearer` prefix; a bad one returns HTTP 401 with a
base64-decode message). 6 endpoints — `ny / germany / amsterdam / uk / tokyo` plus the `global`
edge that routes to the nearest POP. **There is no LA endpoint**: `la.solana.dex.blxrbdn.com`
appears nowhere in their docs and resolves to the same IP as `ny`, so the entry we used to carry
was a duplicate send to New York. Keep-alive is `GET /health` (returns `ok`, no auth). Moved to
port 80 / no TLS so it stays eligible for the io_uring batch writer — rustls owns its record
framing, so a TLS endpoint costs one write syscall per region; the cost is the auth header
crossing the wire in clear text.

Two live traps here:

* **Tip addresses.** The published table has 17. `95cfoy47...` is absent from bloXroute's docs
  entirely and `HWEoBxYs...` survives only in SDK samples that say "as of 2/12/2024 … check docs
  for the latest tip wallet" — both were at the head of our list. A tip to a superseded address
  still leaves the wallet and buys nothing.
* **DNS.** bloXroute picks the datacenter from EDNS Client Subnet. On Cloudflare `1.1.1.1` the
  client ASN is hidden and a region hostname can resolve to a distant POP. The box must resolve
  through Google `8.8.8.8` or OpenDNS. Nothing in `bootstrap.sh` sets this yet — see `INFRA.md`.

**`submitProtection` is now pinned to `SP_LOW`.** Their default, `SP_MEDIUM`, *holds* a
transaction until four consecutive slots are clear of a leader they score as high-risk —
fine for a swap, fatal for a create-block snipe that dies at slip 2. Sending no field means
taking the delaying default, which is what we were doing. `"low"` is rejected with
`Failed to parse request`; the enum spelling `SP_LOW` is the one `/api/v2/submit` parses.
This needed its own `BodyFormat::WrappedBlox` because nextblock shares the `wrapped` body and
must not get the field.

**`useStakedRPCs: true` is now set too, and it is the field that earns bloxroute its race
slot.** Their docs describe `SP_LOW` as "no MEV protection; direct submission to Jito" — so
with SP_LOW alone bloxroute was a slower path to a block engine we already hit ourselves,
plus a hop. `useStakedRPCs` switches it to weighted-stake QoS submission straight to the
leader. It has two documented preconditions and the body already met both: a tip ≥ 0.001 SOL
and `frontRunningProtection=false`. Verified live rather than assumed — the field parses,
which is not obvious given bloXroute rejects unknown fields outright:

```
{... "useStakedRPCs":true ...}  -> 400 "failed to get signature from transaction string: ..."
{... no such field ...}         -> 400 "failed to get signature from transaction string: ..."
{"totallyNotAField":true}       -> 400 "Failed to parse request"          <- the control
```

The first two get as far as decoding the transaction; only the control fails at the envelope.

**blockrazor** is keyed and live on **binary submission**: `POST /v2/sendBinaryTransaction`
with the signed transaction as raw bytes under `application/octet-stream` — no base64, no JSON
envelope, ~26% fewer bytes than `/sendTransaction` and no encode in the hot path
(`BodyFormat::Binary`). Verified by posting one identically-serialised transaction as binary
and as base64 JSON and getting the same downstream error from the provider, so the framing is
right — raw bytes, no length prefix.

* The key must appear **twice**: `?auth=` in the query, which the submit path reads, and the
  `apikey` header, which `/health` requires (it 403s without it). `gen-config` emits both
  because the spec keeps `Auth::Header("apikey")` *and* a `{KEY}` in the path.
* **Plain HTTP on port 443** — that is genuinely what blockrazor publishes, not a typo, and
  TLS there fails the handshake.
* 11 endpoints, not 7: Frankfurt runs three datacenters (`frankfurt`, `-allnodes`,
  `-cherryservers`) and Amsterdam two, plus a Toronto region we were missing. Each is its own
  race entry.
* Min tip is **100,000 lamports (0.0001 SOL)**, the lowest of any provider here; the spec
  previously claimed 1,000,000. Confirmed by the provider's own rejection message.

**flashblock** is keyed and live: bare `Authorization` header, `/api/v2/submit-batch` with
the `{"transactions":[...]}` body, 7 nodes (`ny slc ams fra singapore london tokyo`), min tip
0.0001 SOL. Keep-alive is `GET /`.

**0slot** submits on **`/txb`** (Binary-Tx: raw transaction bytes, no base64, no JSON, no
special headers), not the old `/` JSON-RPC route. Two further traps here:

* **Its keep-alive must NOT carry the key.** `health_path` was `/?api-key={KEY}`, and 0slot
  rate-limits at **5 TPS** on the standard plan, so an authenticated probe every few seconds
  spent submission budget on nothing. Their docs name `/health` for this and say a keyless
  request "does not count toward TPS calculations".
* **Four of its five published hostnames are Cloudflare.** `ny`, `ams`, `jp` and `la` all
  resolve to one anycast pair (172.66.40.254 / 172.66.43.2) — a proxy hop in front of the
  submission host, the same trap as nozomi's digit-less aliases. Measured probe times make the
  cost obvious: `de2` (direct) 21 ms, `ams` (proxied) 29 ms, `ny` (proxied) 206 ms vs `ny2`
  (direct) 106 ms, `la` 338 ms, `jp` **543 ms** — against 237 ms for a direct Tokyo host at
  another provider. Where a direct host exists we now use it (`de2`, `ny2`, both bare metal on
  plain :80). There is no published `ams2`/`jp2`/`la2`; **ask 0slot for the direct names** —
  as they stand, `jp` and `la` will not win a race. Tip list also went 5 → all 21 published
  accounts (the old five were valid, just concentrated).

**astralane** submits on **`/irisb`** (`application/octet-stream`, raw transaction body,
operation chosen by a `method=` query parameter instead of a JSON envelope) rather than
`/iris`. Their own reasoning matches ours: it removes "Base64 Encoding/Decoding Overhead" and
"Packet Splitting due to reduced data size". Deliberately left off: `mev-protect=true` and
`swqos-only=true`, both default false — the first routes around validators (costs a slot), the
second narrows to one path. Regions went 6 → 10: Frankfurt and Amsterdam each run a second
datacenter (`fr2`, `ams2` on Cherry Servers) and Limburg and Lithuania are their own metros.
`edge.astralane.io` is deliberately absent despite their docs recommending it — it resolves to
Cloudflare, so it is a proxy hop, not a submission host. (bloxroute's `global` is kept because
it is the opposite case: it answers on five of bloXroute's own addresses.)

**Its keep-alive probe was destroying the connection.** `health_path` was `/iris?api-key=…`,
and the probe is a GET — a GET to `/iris` answers `400 Bad Request` with **`Connection:
close`**, so every tick tore down the connection it existed to preserve and astralane paid a
fresh TCP handshake on every launch. It is now `/irisb?api-key={KEY}&method=getHealth`, which
answers `405 Method Not Allowed` with `Connection: keep-alive` and an empty body. The status
is irrelevant (jito and nextblock 404 by design); the `Connection` header is the whole point.

This is only visible with `--reuse-after` — a plain reachability run reported all ten
endpoints healthy the entire time, because closing a connection is not an error. **After any
`health_path` change, run `ping_providers.py --reuse-after 6`, not just the default probe.**

**nextblock** carries 9 regions, not 8 — `vilnius.nextblock.io` (88.216.197.109) was missing.

**falcon-udp** is a second falcon entry on their native UDP `:9000`: one datagram of
`16-byte raw UUID || transaction`, no HTTP, no envelope. It **ships disabled**
(`default_regions: ""`, set `FALCON_UDP_REGIONS` to enable) because the transport never
answers, so a wrong key or a truncated datagram is indistinguishable from a successful send —
it cannot be verified except by a funded live fire under `SNIPER_TEST_MODE=1`. It runs
*alongside* the TCP `/binary` entry rather than replacing it; both carry the same durable
nonce, so at most one lands. The connected `UdpSocket` takes an ordinary write, so its regions
still batch through io_uring like any plain-TCP provider.

**lucum and lunar lander (hellomoon) were deleted from the catalogue** on 2026-08-26. A test
pins them out; re-adding one means restoring its `ProviderSpec` and re-verifying its tip list,
not pasting a key into `.env`.

### DNS and reconnects are off the hot path (2026-08-26)

`Endpoint::connect` used to call `to_socket_addrs` — a blocking getaddrinfo — and then
`connect_timeout(5s)`, and it was called from the launch loop, serially, for every endpoint,
*before* a single byte was written. One blackholed region delayed every other region of the
same provider; blockrazor has eleven endpoints on one thread, and the serial retry after the
batch could pay it twice. A create-block snipe is long dead by then.

Now: every hostname resolves once at startup into a cached `SocketAddr`; a launch **skips** a
cold endpoint (`metrics.skipped_cold`) instead of reconnecting inline; the keep-alive tick is
where dead connections get rebuilt, off the critical path; and `CONNECT_TIMEOUT` is 2 s and
applies only there. Note for deployment: resolving once makes bloXroute's EDNS-client-subnet
POP choice sticky for the process lifetime, which is what we want — but a box pointed at
Cloudflare `1.1.1.1` pins the *wrong* POP until restart. See `INFRA.md`.

**falcon (Corvus Labs)** is keyed and live: UUID in `?api-key=`, 9 regions
(`fra ams lon nyc tyo dub sgp slc sqq` — the last is Siauliai, LT, its own metro), min tip
0.001 SOL, keep-alive `GET /health` (no key needed). Submits via **`/binary`**, the same
`BodyFormat::Binary` blockrazor uses. Their tip rule is the strictest of any provider — ONE
top-level System `transfer` (not `transferWithSeed`, not a CPI, not split across two
instructions) of >=1,000,000 lamports to a `Fa1con...` account that appears in the STATIC
account keys, never via a lookup table. Our template's instruction 1 already satisfies all
four. Falcon also caps a transaction at 1,232 bytes; ours is ~1,043, so a template change
could close that gap silently.

Two faster falcon transports exist and are deliberately NOT wired: native UDP on `:9000`
(one datagram of `16-byte raw UUID || transaction`, no envelope, **no reply ever**) and QUIC
on `:5000` via their `falcon-client` SDK. UDP needs a non-stream variant of `Conn`, and since
it answers nothing there is no way to verify it short of a funded live fire. The gain is
mostly *tail* latency — a warm `TCP_NODELAY` socket already emits one segment, so the median
difference from `/binary` is small; what UDP avoids is a TCP stall or retransmit.

### Keep-alive: the interval is set by helius, and it was wrong

Each endpoint is held open by a periodic GET to the provider's `health_path`. **A provider
with an empty `health_path` is never probed at all**, so its connections go cold and every
launch pays a TCP handshake on the hot path — silently, because reconnecting is not an error.
jito, helius-sender, nextblock and flashblock were all in that state.

Idle windows, measured with `scripts/ping_providers.py --reuse-after N` (it holds a real
connection open and probes again), not assumed:

| provider | idle window |
|---|---|
| **helius-sender** | **10 s** (survives 9, gone at 10) |
| flashblock | 30 s (documented; survived 31 in practice) |
| nozomi | 65 s (documented) |
| everything else | longer |

`KEEPALIVE_SECS` in `sender.rs` was **50 s**, so helius-sender's warm connections were always
dead. It is now **4 s**, and the deadline is **absolute**. It used to be armed only after the
spin window expired, which silently added `sender_spin_micros` (2 s in `.env`) to every gap
and made the real interval ~8 s — clearing helius's measured 10 s drop by very little and
blowing straight through the 5 s they actually document ("use connection warming when your
application has gaps longer than 5 seconds"). Worse, the safe probe interval depended on an
unrelated CPU-tuning knob. `next_probe` is now independent of the spin window. The probe was
also
rewritten to write all endpoints then drain replies non-blocking, via the existing
`drain_ready`: the old version did a blocking read per endpoint, which at 11 endpoints could
park a sender thread for over a second — tolerable at a 50 s interval, not at 6 s, and a
launch arriving in that window would have waited behind it.

A non-200 is fine. jito publishes no health endpoint (every path 404s) and nextblock's
`/health` 401s for our key; both answer `/` with a 404 on a connection they keep open, which
is all the probe needs. `every_provider_declares_a_keepalive_path` pins that none is empty.

`scripts/ping_providers.py` probes every endpoint in the generated `sniper.json` and exits
non-zero on any failure or any provider missing a `health_path`:

```
python scripts/ping_providers.py                   # reachability, 81/81 with 0slot disabled
python scripts/ping_providers.py --reuse-after 8   # connections survive the probe interval
python scripts/ping_providers.py --reuse-after 10  # helius-sender should drop — proves the window
```

**A green ping run does NOT mean a provider works.** It probes `health_path`, which for most
providers needs no key at all — 0slot's `/health` returns `OK` without one. Both dead keys
below sat at "healthy" indefinitely. To check that a provider will actually accept a submit,
POST a deliberately malformed transaction with the real key and read the *body*: a complaint
about the transaction means auth passed, an auth error means it did not.

```
0slot      real key -> {"error":{"code":403,"message":"api-key does not exist"}}   # dead
0slot      bogus    -> identical
node1      real key -> "Decode Transaction Error: no signature"  (605)             # live
node1      bogus    -> "Invalid Api-Key" (401)
```

Re-run after any `.env` edit (WSL, not Windows):

```
cd shred-sniper && cargo run -p sniper --bin gen-config -- --env ../.env --out sniper.json
```

## Buy instruction: buy_exact_sol_in (aligned with the leader, 2026-08-26)

Decoded from E4Ez's 439 own-signed buys (feePayer = E4Ez): **74% use `buy_exact_sol_in`**
(fix the SOL, floor the tokens), 26% classic `buy` via a router. His token slippage: median
**6.4%** (p25 4.6%, p75 11%). NOTE: the `proVF4pMXVaYqmy4NjniPh4pqKNfMmsihgd4wdkCX3u` router
program seen earlier was OTHER wallets delivering to him, not his own buys - his own buys call
pump directly.

Switched our buy path to `buy_exact_sol_in` (`SNIPER_BUY_EXACT_SOL_IN=1`, default on). Verified
his instruction uses the SAME 18-account layout as `buy`, so the change is localized: template
swaps the discriminator (`[56,252,116,8,158,223,205,95]`) and the two u64s become (sol_in,
min_tokens_out) at the same offsets. `CurveParams::plan_exact_sol_in` sets sol_in = budget,
min_tokens = expected × (1 − slippage). Why it's better for us: the spend is fixed (never
overshoots) and there's NO exact-token depth computation whose error blows `max_sol_cost` - the
depth estimate only sets the floor. `SNIPER_SLIPPAGE_BPS` = 650 (his median), and its meaning is
now instruction-dependent: exact_sol_in → min-tokens floor below expected; buy → max_sol_cost
headroom. Both revert on a ~2-buy up-move (slip-0/1 fills, later reverts). Test:
`pumpfun::exact_sol_in_fixes_spend_and_floors_tokens`. **MUST be validated in SNIPER_TEST_MODE=1
(one real buy) before live** - a wrong discriminator/arg = a failed buy.

## Money-path audit round 3 (2026-08-26): exact_sol_in cost-basis P0

The `buy_exact_sol_in` switch silently redefined `plan.amount`/`plan.max_sol_cost` (arg1
became a TOKEN floor, ~1e13), and three consumers kept reading the old lamport meaning:

* **Seller cost basis** (`src/pump.ts`): `entryCostLamports` came from `Fill.max_sol_cost`
  → token count as cost → value multiple read ~0 → the 0.8× stop dumped every position at
  slot +1. The certified ladder was silently degraded to dump@1.
* **Ghost PnL** (`lib.rs`): cost = `max_sol_cost/(1+slip)` and `OpenPosition.amount` =
  lamports priced as tokens — re-introduced the round-2 ghost-cost bug. Ghost soak numbers
  from before this fix are garbage under exact mode.
* **Dynamic tip sizing** (`lib.rs`): size term fed `plan.max_sol_cost` → token floor →
  tip pinned at its max clamp every launch.

Fix: `BuyPlan` now carries instruction-INDEPENDENT `budget_lamports` + `expected_tokens`
(set by both planners); `FiredLaunch` carries `cost_lamports` + `expected_tokens`;
`OpenPosition.{cost,amount}`, tip sizing, and the proxy's `fill_from_fired()` all read those.
On the Fill wire, `max_sol_cost` = cost basis in lamports, `amount` = expected tokens — for
both instructions (proto comments updated); pump.ts uses the field directly, slippage
back-out deleted. Regression tests at every hop: `pumpfun::both_planners_carry_the_
instruction_independent_cost_basis`, `lib::fired_launch_cost_basis_is_lamports_under_both_
buy_instructions` (fires a ghost HotSniper through both instructions),
`forwarder::fill_carries_cost_basis_and_expected_tokens_not_the_wire_args`, and the
"cost basis units (exact_sol_in regression)" block in `src/pumpFun/ladder.test.ts`.

Also from this audit, not yet fixed: a hard-down provider region reconnects inline on the
send path (`sender.rs` — blocking DNS + 5 s connect timeout per launch, delaying that
provider's healthy regions); wants a per-endpoint reconnect backoff.

## Standing caveats

The +0.5/trade figures come from a self-selected population of good operators. Copying E4Ez's
parameters does not guarantee reproducing his edge. Paper-trade before risking capital.

The simulator and the real-trade data **disagree on entry timing**. Where they conflict, trust
the real trades.
