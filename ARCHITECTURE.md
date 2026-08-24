# Layout

```
.                     node process: sells, analytics, telegram
├── src/              websocket-driven, nothing latency critical
└── shred-sniper/     rust workspace: shred ingest, detection, buying
    ├── proxy/        jito shredstream proxy (deshredder, gRPC feeds)
    └── sniper/       detection, transaction template, provider senders
```

## One process does the buying

The buy path never crosses a process boundary, a socket, or a channel:

```
udp shred
  └─ ssListen{n}                     proxy/src/forwarder.rs   streamer::receiver
      └─ shred_reconstructor         proxy/src/forwarder.rs   THE ONLY HOT THREAD
          ├─ deshred::reconstruct_shreds
          ├─ pumpfun::parse_create          direct call
          ├─ HotSniper::on_create           direct call, same thread, same stack
          │    whitelist → dedup → 3 PDA derivations → patch template bytes
          └─ crossbeam try_send → snipeTx_<provider>   sign, base64, socket write
```

`sniper` is a **library crate compiled into the proxy binary**, not a service. `on_create`
is an ordinary function call from the thread that just deshredded the launch. There is no
serialization, no IPC and no scheduler hop between detecting a launch and having bytes
queued for the wire.

The only work handed off is the send itself, and that is deliberate: signing is ~20µs of
ed25519 and a socket write can block. Both happen on per-provider threads so a wedged
provider cannot stall deshredding. The handoff is a bounded crossbeam channel — a memcpy
into a preallocated slot, no allocation.

Two things do cross a process boundary, both far away from the buy:

| Feed | Consumer | Why it is safe |
| --- | --- | --- |
| `PumpCreate` gRPC | analytics | emitted *after* the buy is already queued |
| `Fill` gRPC | node seller | selling happens seconds later |

The node process cannot slow the buy path down, because it is not on it.

## Why the code used to be split

Buying lived in Node: a shredstream entry arrived as base64 over gRPC, was decoded with
web3.js, checked against redis (two awaits), and built with Anchor. That path had a real
cross-process hop and it is the thing that was removed. What is left in Node is the part
where milliseconds do not matter.

## Building

The repository path contains a space, which breaks `openssl-sys`'s vendored build (its
Makefile does not quote paths). Build with a target directory that has none:

```bash
export CARGO_TARGET_DIR=$HOME/sniper-target
```

```bash
# rust side (needs the 1.87 toolchain pinned in shred-sniper/rust-toolchain.toml)
cd shred-sniper
cargo build --release
cargo test -p sniper
cargo run -p sniper --bin gen-config -- --env ../.env    # writes sniper.json
cargo run -p sniper --example dump_offsets               # byte patch table

# node side
npm install
npm run build:napi     # the native addon, must be rebuilt on the target OS
npm run build
```

Enable the SIMD backend for ed25519 while you are at it — it is a build flag, not a code
change, and takes several microseconds off every signature:

```bash
RUSTFLAGS="-C target-cpu=native" cargo build --release
```

See `shred-sniper/SNIPER.md` for the transaction layout, provider table, sizing maths and
deployment notes.
