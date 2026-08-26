#!/bin/bash
# Compile + test the whole shred-sniper workspace under the Linux toolchain (WSL/dev box).
set -uo pipefail
[ -f "$HOME/.cargo/env" ] && source "$HOME/.cargo/env"
cd "/mnt/f/Solana sniper/shred-sniper"
export CARGO_TARGET_DIR=/tmp/sniper-target
export OPENSSL_NO_VENDOR=1
export RUSTFLAGS="-C target-cpu=native"
echo "=== confirm + tip module tests ==="
cargo test -p sniper 2>&1 | grep -E "test (confirm|tip)::|test result: FAILED|test result: ok"
echo "=== full workspace check (proxy + sniper) ==="
cargo check --workspace 2>&1 | grep -E "error\[|^error:|^warning:|Finished" | head -40
echo "WSL_EXIT=${PIPESTATUS[0]}"
