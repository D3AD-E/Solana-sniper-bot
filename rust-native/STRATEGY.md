# v1.1 confirmation trigger on the buy path

`src/strategy.rs` is the v1.1 decision (frozen 2026-08-25; backtest +346/+503 SOL/day
trimmed on held-out days; live-paper verified). It replaces the creator-whitelist decision
in `selection.rs` for the MAIN book — `selection.rs` stays for its curve math, which
`strategy.rs` imports. No napi, no solana-sdk; drops into the shred proxy like selection.

All tunables come from `.env` (see the "v1.1 confirmation-trigger strategy" block there);
`Params::from_env()` reads them once at startup with the frozen values as compiled-in
defaults. `SNIPER_CU_LIMIT=96000` is measured: consumed p50 73k / p99 87k on 149 of the
leader's landed buys (`analysis/cu_measure.py`); you pay `cu_price x cu_limit` regardless,
so the limit is p99 + margin, not a round number.

## Wiring

At startup:

```rust
let params = strategy::Params::from_env();
strategy::load_devs_from_file("dev_history.txt")?;      // fail fast on missing tables
strategy::load_watch_from_file("watch_wallets.tsv")?;
strategy::spawn_reloader("dev_history.txt".into(), "watch_wallets.tsv".into(),
                         Duration::from_secs(60));
let mut session = strategy::Session::default();
```

On a decoded pump `create`:

```rust
if !session.dev_is_fresh(&creator) { return; }          // 64-78% die here (measured live)
let mut lw = strategy::LaunchWatch::new(dev_buy_net_lamports);
pending.insert(mint, lw);                               // keyed by mint, create block only
```

On each decoded create-block trade of a pending mint (in landing order):

```rust
match lw.on_buy(&buyer, net_lamports, &params) {        // sells: lw.on_sell(net)
    Ok(fire) => {
        // fire.token_amount / fire.max_sol_cost go straight into the pump buy ix.
        // max_sol_cost == position size: an adverse move past it reverts on-chain
        // (6002) and costs only the tip - that is the real-money slippage guard.
        fire_buy(mint, fire);
        pending.remove(&mint);
    }
    Err(Pass::Watching) => {}                           // keep feeding
    Err(_) => { pending.remove(&mint); }                // EliteAhead / CurveCapped / dead
}
```

When the create block seals (next slot begins), drop every still-watching entry —
the trigger only exists inside the create block.

Exits: `params.exit_legs` + `exit_last_slot` + `stop_x_bps` / `moon_x_bps` /
`moon_frac_bps` describe the frozen ladder; execution lives on the sender/position side.

## Tables

* `dev_history.txt` — one deployer per line. A dev on the list is NOT fresh. Kept current
  two ways: `analysis/dev_sweep.py` (daemon, PumpPortal creates -> Postgres -> flat file
  every ~5 min; installed as scheduled task `pump-dev-sweep` on Windows,
  `scripts/dev-sweep.service` on Linux) and `Session::dev_is_fresh`, which marks every
  dev the proxy itself sees, so freshness holds even between reloads.
* `watch_wallets.tsv` — `<wallet> <follow_symbiont|follow_whale|avoid>` from the
  `watch_wallets` Postgres table (`analysis/build_watchlists.py` rebuilds it from tape
  days; symbionts die with their creator, whales rotate within ~a week — rebuild after
  every new extracted day).

## Verifying a change

Same standalone flow as selection, but two files (strategy imports selection's curve
math). Run it in WSL — the prod toolchain:

```sh
mkdir -p /tmp/selstrat/src
cp rust-native/src/{selection,strategy}.rs /tmp/selstrat/src/
printf 'pub mod selection;\npub mod strategy;\n' > /tmp/selstrat/src/lib.rs
printf '[package]\nname="selstrat"\nversion="0.1.0"\nedition="2021"\n\n[dependencies]\narc-swap="1"\n' > /tmp/selstrat/Cargo.toml
cd /tmp/selstrat && cargo test    # 17 tests
```

The tests pin: fire on the 3rd buy only once >=4 SOL confirmed, sizing clamps + 0.1 snap,
the vq>=115 bundle-sweep guard (blocked 4 real sweeps in one live session, one of them
-3.26 SOL in paper), elite-ahead kill, follow-wallet immediate fire, and once-per-session
dev freshness.
