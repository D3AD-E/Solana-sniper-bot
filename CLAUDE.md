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

## Standing caveats

The +0.5/trade figures come from a self-selected population of good operators. Copying E4Ez's
parameters does not guarantee reproducing his edge. Paper-trade before risking capital.

The simulator and the real-trade data **disagree on entry timing**. Where they conflict, trust
the real trades.
