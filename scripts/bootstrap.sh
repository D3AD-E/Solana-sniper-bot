#!/bin/bash
# Sets up a fresh Ubuntu/Debian box to build and run the sniper.
#
#   sudo ./scripts/bootstrap.sh                 everything
#   sudo ./scripts/bootstrap.sh --deps-only     packages and toolchains, no build
#   sudo ./scripts/bootstrap.sh --no-systemd    skip installing the units
#   sudo ./scripts/bootstrap.sh --tune          also apply the host tuning
#
# Idempotent: safe to re-run. Never writes secrets, never touches the chain, never
# overwrites an existing sniper.json, .env, keypair or whitelist.
set -euo pipefail

REPO_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
RUST_DIR="$REPO_DIR/shred-sniper"
# the repo path may contain a space, which breaks vendored openssl's makefile
TARGET_DIR="${CARGO_TARGET_DIR:-/var/tmp/sniper-target}"
RUN_USER="${SUDO_USER:-$(id -un)}"
RUN_HOME="$(getent passwd "$RUN_USER" | cut -d: -f6)"

DEPS_ONLY=0
DO_SYSTEMD=1
DO_TUNE=0
for arg in "$@"; do
  case "$arg" in
    --deps-only) DEPS_ONLY=1 ;;
    --no-systemd) DO_SYSTEMD=0 ;;
    --tune) DO_TUNE=1 ;;
    *) echo "unknown argument: $arg" >&2; exit 1 ;;
  esac
done

step() { printf '\n\033[1m==> %s\033[0m\n' "$1"; }
note() { printf '    %s\n' "$1"; }
have() { command -v "$1" >/dev/null 2>&1; }

as_user() { sudo -u "$RUN_USER" -H bash -lc "$1"; }

if [ "$(id -u)" -ne 0 ]; then
  echo "run with sudo: sudo $0 $*" >&2
  exit 1
fi

step "Host"
note "repo:        $REPO_DIR"
note "build user:  $RUN_USER"
note "target dir:  $TARGET_DIR"
note "$(. /etc/os-release && echo "$PRETTY_NAME") kernel $(uname -r)"

# io_uring batching needs 5.6+, and everything here assumes systemd
kernel_major=$(uname -r | cut -d. -f1)
kernel_minor=$(uname -r | cut -d. -f2)
if [ "$kernel_major" -lt 5 ] || { [ "$kernel_major" -eq 5 ] && [ "$kernel_minor" -lt 6 ]; }; then
  note "WARNING: kernel < 5.6, io_uring batched writes will fall back to sequential"
fi

step "System packages"
export DEBIAN_FRONTEND=noninteractive
apt-get update -qq
# libssl-dev + OPENSSL_NO_VENDOR means openssl-sys links the system library instead of
# compiling its own, which is faster and sidesteps paths containing spaces
apt-get install -y -qq --no-install-recommends \
  build-essential pkg-config libssl-dev libudev-dev zlib1g-dev \
  clang llvm cmake make g++ \
  protobuf-compiler libprotobuf-dev \
  git curl ca-certificates jq \
  ethtool numactl util-linux chrony \
  linux-tools-common "linux-tools-$(uname -r)" 2>/dev/null || \
apt-get install -y -qq --no-install-recommends \
  build-essential pkg-config libssl-dev libudev-dev zlib1g-dev \
  clang llvm cmake make g++ \
  protobuf-compiler libprotobuf-dev \
  git curl ca-certificates jq \
  ethtool numactl util-linux chrony
note "installed"

systemctl enable --now chrony >/dev/null 2>&1 || true
note "chrony enabled (metrics and log timestamps need a sane clock)"

step "Rust"
if as_user 'command -v cargo' >/dev/null 2>&1; then
  note "already present: $(as_user 'rustc --version')"
else
  as_user "curl -sSf https://sh.rustup.rs | sh -s -- -y --profile minimal --default-toolchain none"
  note "rustup installed"
fi
# rust-toolchain.toml pins the version; this makes rustup fetch it now rather than mid-build
as_user "cd '$RUST_DIR' && rustup show active-toolchain || rustup toolchain install \$(grep -oP 'channel = \"\\K[^\"]+' rust-toolchain.toml)"
# rust-native/ has no toolchain file, so builds outside shred-sniper need a rustup default
as_user "rustup default \$(grep -oP 'channel = \"\\K[^\"]+' '$RUST_DIR/rust-toolchain.toml') 2>/dev/null || rustup default stable" || true
note "toolchain: $(as_user "cd '$RUST_DIR' && rustc --version" 2>/dev/null || echo unknown)"

step "Node.js"
if have node && [ "$(node -v | cut -c2- | cut -d. -f1)" -ge 20 ]; then
  note "already present: $(node -v)"
else
  curl -fsSL https://deb.nodesource.com/setup_22.x | bash - >/dev/null
  apt-get install -y -qq nodejs
  note "installed $(node -v)"
fi

step "Solana CLI"
if as_user 'command -v solana' >/dev/null 2>&1; then
  note "already present: $(as_user 'solana --version')"
else
  as_user "curl -sSfL https://release.anza.xyz/stable/install | sh" >/dev/null 2>&1 || \
    note "install failed; only needed for creating nonce accounts and keypairs"
  note "installed (add \$HOME/.local/share/solana/install/active_release/bin to PATH)"
fi

if [ "$DEPS_ONLY" -eq 1 ]; then
  step "Done (--deps-only)"
  exit 0
fi

step "Build: rust"
mkdir -p "$TARGET_DIR"
chown -R "$RUN_USER" "$TARGET_DIR"
as_user "cd '$RUST_DIR' && \
  export CARGO_TARGET_DIR='$TARGET_DIR' OPENSSL_NO_VENDOR=1 && \
  export RUSTFLAGS='-C target-cpu=native' && \
  cargo build --release"
install -m 0755 "$TARGET_DIR/release/jito-shredstream-proxy" "$REPO_DIR/jito-shredstream-proxy"
note "built $(du -h "$REPO_DIR/jito-shredstream-proxy" | cut -f1) binary"

step "Build: node seller"
as_user "cd '$REPO_DIR' && npm ci --silent 2>/dev/null || npm install --silent"
# the native addon is platform specific and must be built here, not shipped
as_user "cd '$REPO_DIR/rust-native' && \
  export CARGO_TARGET_DIR='$TARGET_DIR/napi' OPENSSL_NO_VENDOR=1 && \
  npx --yes @napi-rs/cli@2 build --release"
# npm run build = tsc + copying proto/, generated/ grpc stubs and the napi addon into dist —
# bare tsc leaves dist without them and the seller dies with MODULE_NOT_FOUND
as_user "cd '$REPO_DIR' && npm run build"
note "seller built"

step "Configuration"
if [ ! -f "$REPO_DIR/.env" ]; then
  note "no .env found. Copy your keys in, then re-run to generate sniper.json:"
  note "  cp .env.example .env && \$EDITOR .env"
else
  if [ -f "$RUST_DIR/sniper.json" ]; then
    note "sniper.json exists, leaving it alone (delete it to regenerate)"
  else
    as_user "cd '$RUST_DIR' && CARGO_TARGET_DIR='$TARGET_DIR' cargo run -q -p sniper --bin gen-config -- --env ../.env --out sniper.json"
    chmod 600 "$RUST_DIR/sniper.json"
    note "wrote sniper.json (chmod 600, contains API keys)"
  fi
fi

[ -f "$RUST_DIR/whitelist.txt" ] || {
  cp "$RUST_DIR/whitelist.example.txt" "$RUST_DIR/whitelist.txt"
  chown "$RUN_USER" "$RUST_DIR/whitelist.txt"
  note "created an empty whitelist.txt — nothing fires until it has launchers in it"
}

step "Self-test"
as_user "cd '$RUST_DIR' && CARGO_TARGET_DIR='$TARGET_DIR' cargo test --release -p sniper -q 2>&1 | tail -3" || true
note "byte patch layout:"
as_user "cd '$RUST_DIR' && CARGO_TARGET_DIR='$TARGET_DIR' cargo run -q -p sniper --example dump_offsets 2>/dev/null | head -4" || true
note "this machine's signing speed (lower is better, ~11us here, good boxes beat 8us):"
as_user "cd '$RUST_DIR' && CARGO_TARGET_DIR='$TARGET_DIR' cargo test --release -p sniper bench_sender_stages -- --nocapture --test-threads=1 2>/dev/null | grep ed25519" || true

if [ "$DO_SYSTEMD" -eq 1 ]; then
  step "systemd units"
  for unit in sniper-proxy sniper-seller sniper-latency; do
    src="$REPO_DIR/scripts/systemd/$unit.service"
    [ -f "$src" ] || continue
    sed -e "s|@REPO@|$REPO_DIR|g" -e "s|@USER@|$RUN_USER|g" "$src" > "/etc/systemd/system/$unit.service"
    note "installed $unit.service"
  done
  systemctl daemon-reload
  systemctl enable --now sniper-latency >/dev/null 2>&1 || true
  note "sniper-latency started (holds /dev/cpu_dma_latency at 0, keeps cores out of deep C-states)"
  note "start the rest once configured:  systemctl start sniper-proxy sniper-seller"
fi

if [ "$DO_TUNE" -eq 1 ]; then
  step "Host tuning"
  bash "$RUST_DIR/scripts/tune.sh"
fi

step "Next"
cat <<EOF
    1. keys:      edit .env, then delete shred-sniper/sniper.json and re-run this script
    2. wallet:    solana-keygen new -o /opt/sniper/keypair.json   (chmod 600)
                  point SNIPER_KEYPAIR at it, fund it
    3. nonces:    one per concurrent buy, four is a sensible start
                  solana-keygen new --no-bip39-passphrase -o nonce1.json
                  solana create-nonce-account nonce1.json 0.0015 --nonce-authority <buyer>
                  put the addresses in SNIPER_NONCE_ACCOUNTS
    4. whitelist: put launcher pubkeys in shred-sniper/whitelist.txt, one per line
    5. boot args: isolcpus=6,7 nohz_full=6,7 rcu_nocbs=6,7 processor.max_cstate=1
                  then reboot and run scripts/tune.sh --pin
    6. prove it:  SNIPER_GHOST_MODE=1 first, then SNIPER_TEST_MODE=1, then live

    INFRA.md has the reasoning behind each of these.
EOF
