# Latitude bare-metal deploy (Ubuntu)

Target: one Latitude box in **FRA** (Frankfurt — nearest Jito block engine region to
`NODE_REGION=fra`; Amsterdam is the pair). Everything below is idempotent.

## First time

```sh
# 0. from the dev machine (WSL): ship the repo
./scripts/push.sh user@box                    # SNIPER_DEST=/opt/sniper by default

# 1. on the box: base toolchain, builds, systemd units, host tuning
sudo /opt/sniper/scripts/bootstrap.sh --tune

# 2. on the box: v1.1 layer - postgres :5433, strategy tables, dev-sweep service
sudo /opt/sniper/scripts/deploy_v11.sh

# 3. secrets & accounts (bootstrap prints the same list):
#    - .env is synced by push.sh; verify keys, then regenerate the provider config:
cd /opt/sniper/shred-sniper && cargo run -q -p sniper --bin gen-config -- --env ../.env
#    - wallet keypair at SNIPER_KEYPAIR (/etc/sniper/keypair.json, chmod 600), funded
#    - nonce accounts in SNIPER_NONCE_ACCOUNTS (one per concurrent buy)
#    - Jito ShredStream auth keypair (see below - the long pole)

# 4. prove it before money:
SNIPER_GHOST_MODE=1  ->  soak, compare ghost PnL to analysis/data/live_paper.json rates
SNIPER_TEST_MODE=1   ->  one real round trip
live                 ->  size_hi low first, raise after a clean day
```

## Fast iteration loop

```sh
# dev machine, after any change:
./scripts/push.sh user@box --restart          # rsync + cargo build on box + restart units
# strategy tunables need no rebuild at all - they are .env; tables hot-reload every 60 s
```

Three change classes, three speeds:

| change | action |
| --- | --- |
| strategy knobs (sizes, trigger, ladder, tips) | edit `.env` on the box, restart seller only |
| tables (watchlist, dev history) | nothing — dev-sweep + reloader pick them up live |
| rust/ts code | `push.sh --restart` (~1 min incl. incremental build) |

## What runs where

| unit | what |
| --- | --- |
| `sniper-proxy` | shred ingest + v1.1 decision (strategy.rs) + fire |
| `sniper-seller` | exits: ladder legs, stop, force-out |
| `sniper-latency` | holds cpu_dma_latency at 0 |
| `dev-sweep` | keeps dev_history fresh (PumpPortal WS -> postgres -> flat files) |
| `postgresql` | :5433, db `pumpinfo` — watch_wallets + dev_history |

Health: `psql ... -c "select max(updated_at) from dev_history"` — older than 10 min
means the freshness filter is rotting; halt fires until dev-sweep is back.

## Region & leader routing

* Detection colo is what buys slip-1; submission fan-out is already solved: every
  provider fires at its `*_REGIONS` endpoints and all variants share one durable nonce,
  so only one can land and only one tip is paid. Keep `JITO_REGIONS=all` or trim to
  `fra,ams` to cut egress — measure landed-rank per region after a week and trim to
  what wins.
* Leader tracking: not needed for the Jito path (the block engine relays to the current
  leader wherever it is). It becomes worth it only if a direct-TPU path is added —
  then: getLeaderSchedule per epoch, gossip TPU addresses, QUIC to current + next two
  leaders. Park it until the Jito path is measured.
