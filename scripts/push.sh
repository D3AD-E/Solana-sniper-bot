#!/bin/bash
# Fast-iteration push from the dev machine to the Latitude box.
#
#   ./scripts/push.sh user@box                 sync only
#   ./scripts/push.sh user@box --build         sync + rebuild rust on the box
#   ./scripts/push.sh user@box --restart       sync + rebuild + restart services
#
# Run from WSL or any shell with rsync+ssh. Excludes build output, node_modules and the
# heavy replay parquets; .env IS synced (chmod 600 on the far side) since the box needs
# the keys - remove the line if you manage secrets another way.
set -euo pipefail

HOST="${1:?usage: push.sh user@host [--build|--restart]}"
MODE="${2:-}"
DEST="${SNIPER_DEST:-/opt/sniper}"
SRC="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"

rsync -az --delete --info=stats1 \
  --exclude .git --exclude node_modules --exclude target --exclude dist \
  --exclude 'analysis/data/replay*' --exclude 'analysis/data/*.jsonl' \
  --exclude '*.lock' --exclude 'shred-sniper/sniper.json' \
  "$SRC/" "$HOST:$DEST/"
ssh "$HOST" "chmod 600 '$DEST/.env' 2>/dev/null || true"
# sniper.json is generated from .env and carries API keys - never clobber the box's copy with a
# stale dev one, and regenerate it from the just-synced .env so json + env agree. (The proxy
# also applies .env overrides at load, so scalar knobs are correct even before this runs.)
ssh "$HOST" "cd '$DEST/shred-sniper' && CARGO_TARGET_DIR=/var/tmp/sniper-target \
  cargo run -q -p sniper --bin gen-config -- --env ../.env --out sniper.json 2>/dev/null || \
  echo 'note: gen-config skipped (run bootstrap once first)'"
echo "synced -> $HOST:$DEST"

if [ "$MODE" = "--build" ] || [ "$MODE" = "--restart" ]; then
  ssh "$HOST" "cd '$DEST/shred-sniper' && \
    CARGO_TARGET_DIR=/var/tmp/sniper-target OPENSSL_NO_VENDOR=1 \
    RUSTFLAGS='-C target-cpu=native' cargo build --release -q && \
    install -m 0755 /var/tmp/sniper-target/release/jito-shredstream-proxy '$DEST/jito-shredstream-proxy'"
  echo "rebuilt proxy"
fi
if [ "$MODE" = "--restart" ]; then
  ssh "$HOST" "sudo systemctl restart sniper-proxy sniper-seller dev-sweep && \
    systemctl is-active sniper-proxy sniper-seller dev-sweep"
  echo "restarted"
fi
