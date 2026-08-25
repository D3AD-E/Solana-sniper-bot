#!/bin/bash
# v1.1 layer on top of bootstrap.sh, for the Latitude Ubuntu box. Idempotent.
#
#   sudo ./scripts/deploy_v11.sh
#
# Does: postgres install + pumpinfo db/user, strategy tables restore, python deps,
# flat-table export, dev-sweep systemd unit. Run AFTER scripts/bootstrap.sh.
set -euo pipefail

REPO_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
RUN_USER="${SUDO_USER:-$(id -un)}"

step() { printf '\n\033[1m==> %s\033[0m\n' "$1"; }
note() { printf '    %s\n' "$1"; }

[ "$(id -u)" -eq 0 ] || { echo "run with sudo" >&2; exit 1; }

step "Postgres"
export DEBIAN_FRONTEND=noninteractive
apt-get install -y -qq postgresql python3-pip python3-venv >/dev/null
systemctl enable --now postgresql
# the analysis code expects port 5433; move the cluster there if it is on 5432
PGCONF=$(ls /etc/postgresql/*/main/postgresql.conf | head -1)
if ! grep -q '^port = 5433' "$PGCONF"; then
  sed -i 's/^port = .*/port = 5433/' "$PGCONF"
  systemctl restart postgresql
  note "moved postgres to :5433"
fi
sudo -u postgres psql -p 5433 -tc "select 1 from pg_roles where rolname='pumpinfo'" | grep -q 1 || \
  sudo -u postgres psql -p 5433 -c "create role pumpinfo login password 'pumpinfo'"
sudo -u postgres psql -p 5433 -tc "select 1 from pg_database where datname='pumpinfo'" | grep -q 1 || \
  sudo -u postgres createdb -p 5433 -O pumpinfo pumpinfo
note "db ready: postgresql://pumpinfo:pumpinfo@localhost:5433/pumpinfo"

step "Strategy tables"
if [ -f "$REPO_DIR/scripts/db/watchlists.sql" ]; then
  PGPASSWORD=pumpinfo psql -q -h localhost -p 5433 -U pumpinfo -d pumpinfo \
    -f "$REPO_DIR/scripts/db/watchlists.sql"
  note "restored watch_wallets + dev_history from scripts/db/watchlists.sql"
else
  note "WARNING: scripts/db/watchlists.sql missing - run analysis/db_dump.py on the dev box"
fi

step "Python deps"
sudo -u "$RUN_USER" -H pip3 install --quiet --user --break-system-packages \
  "psycopg[binary]" websockets requests 2>/dev/null || \
sudo -u "$RUN_USER" -H pip3 install --quiet --user "psycopg[binary]" websockets requests
note "installed psycopg, websockets, requests"

step "Flat tables for the proxy"
sudo -u "$RUN_USER" -H python3 "$REPO_DIR/analysis/export_tables.py"

step "dev-sweep service"
sed "s|/opt/sniper|$REPO_DIR|; s|User=.*|User=$RUN_USER|" \
  "$REPO_DIR/scripts/dev-sweep.service" > /etc/systemd/system/dev-sweep.service
grep -q "^User=" /etc/systemd/system/dev-sweep.service || \
  sed -i "/^\[Service\]/a User=$RUN_USER" /etc/systemd/system/dev-sweep.service
systemctl daemon-reload
systemctl enable --now dev-sweep
note "dev-sweep running: $(systemctl is-active dev-sweep)"

step "Done"
cat <<EOF
    verify:  systemctl status dev-sweep --no-pager | head -5
             psql "postgresql://pumpinfo:pumpinfo@localhost:5433/pumpinfo" \\
               -c "select count(*), max(updated_at) from dev_history"
    a stale max(updated_at) (> 10 min) means the freshness filter is rotting - halt.
EOF
