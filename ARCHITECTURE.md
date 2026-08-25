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
      └─ ssPxyTx_{n}                 proxy/src/forwarder.rs   Arc, no copy
          └─ shred_reconstructor     proxy/src/forwarder.rs   THE ONLY HOT THREAD
              ├─ deshred::reconstruct_shreds
              │    └─ detect_in_partial_segments   fires mid-segment, see below
              ├─ pumpfun::parse_create          direct call
              ├─ HotSniper::on_create           direct call, same thread, same stack
              │    whitelist → dedup → 1 PDA derivation → patch template bytes
              │    → one copy into a shared body
              └─ crossbeam try_send → snipeTx_<provider>   patch tip, sign, base64, write
```

`sniper` is a **library crate compiled into the proxy binary**, not a service. `on_create`
is an ordinary function call from the thread that just deshredded the launch. There is no
serialization, no IPC and no scheduler hop between detecting a launch and having bytes
queued for the wire.

The only work handed off is the send itself, and that is deliberate: signing is ~10µs of
ed25519 and a socket write can block. Both happen on per-provider threads so a wedged
provider cannot stall deshredding. The handoff is a bounded crossbeam channel — a memcpy
into a preallocated slot, no allocation.

## Detection does not wait for the segment

The obvious way to read shreds is the wrong one. `Shredder::deshred` will only assemble a
segment that is bounded on both sides by `DATA_COMPLETE_SHRED`, so the ordinary path cannot
say anything about a launch until the *last* shred of the segment around it has arrived. A
segment runs to 79 shreds; a create sitting near its front is invisible for the whole of
that, which is milliseconds. Next to that, the ~5µs the internal path costs is noise.

Nothing actually forces the wait. A segment is a bincode `Vec<Entry>` written in order, so
whatever has arrived is a valid *prefix* of that encoding. `deshred::detect_in_partial_segments`
keeps the bytes each segment has produced so far, appends newly arrived shreds, searches only
the newly appended bytes for a create discriminator, and hands over any create whose
transaction is complete. `wire::transaction_end` already returns `None` rather than guessing
when it walks off the end, so a half-arrived transaction simply is not reported yet.

`a_create_is_found_before_its_segment_ends` pins the behaviour down: in a 79 shred segment
with the create up front, the partial path reports it on **shred 0** and the ordinary path on
shred 78.

The ordinary path still runs and still publishes the whole segment to the entry feed. A
launch reported twice is harmless — `HotSniper` dedups on mint.

## One body, many providers

The fan-out used to copy the whole 1232 byte transaction once per provider, patch that
copy's tip fields, and clone it again into the channel. With twelve providers that is ~44KB
of memcpy on the detect thread, and the twelfth provider was queued roughly 10µs after the
first — with no way to know which one was going to win.

Now the detect thread copies the body once into a pooled `Arc<TxBuf>` and sends each provider
a 56 byte `Job`: the shared body plus its own tip account, tip and compute-unit price. The
sender threads stamp those three fields onto their own copy before signing, which costs
nothing because there is one such thread per provider and they are otherwise idle. Firing at
twelve providers now costs about what firing at one does:

| providers | before | after |
| --- | --- | --- |
| 1  | ~5.4µs | 5.4µs |
| 12 | grows with provider count | 6.5µs |

Two things do cross a process boundary, both far away from the buy:

| Feed | Consumer | Why it is safe |
| --- | --- | --- |
| `PumpCreate` gRPC | analytics | emitted *after* the buy is already queued |
| `Fill` gRPC | node seller | selling happens seconds later |

The node process cannot slow the buy path down, because it is not on it.

## Things that are not obvious from the code

* **Per-slot trackers are pooled.** `ShredsStateTracker` is `MAX_DATA_SHREDS_PER_SLOT` wide,
  which is 4MiB for its `data_shreds` alone. Building one per slot put a fresh mapping and a
  4MiB zeroing on the detect thread every ~400ms — paid by whichever shred opened the slot,
  which is exactly the shred a launch might be in. `TrackerPool` recycles them and clears only
  the indexes that were written.
* **Shreds are rejected before they are parsed.** `new_from_serialized_shred` copies ~1228
  bytes onto the heap and validates the merkle variant. Slot, index and `fec_set_index` sit at
  fixed offsets in the common header, so a duplicate — and with several regions subscribed,
  most arrivals are duplicates — is dropped for the price of reading three fields.
  `peeked_fec_set_index_matches_the_parsed_shred` checks the hardcoded offset against the
  parsed value on every shred in the capture, because getting it wrong would silently drop
  live shreds.
* **The sender never blocks on a provider's response.** It used to read the reply to each
  submit with a blocking socket, which parks the thread for a full network round trip per
  endpoint — eight regions at 20ms is 160ms during which a queued launch does not go out. The
  drain is non-blocking; leftovers are picked up after the next write or on the keep-alive
  tick.
* **Nothing on the buy path talks to a node.** Every `rpc_url` in `sniper` is either in
  `Sniper::start` or on a background refresher thread. Nonce values, fee recipients, the pump
  global config and the whitelist all reach the hot path as an `ArcSwap` load.
* **Token accounts are derived before the first launch, not during one.** The address is
  `sha256(buyer || seed || token_program)`, and none of those three depend on the launch, so
  `SeedTable` computes 65536 of them at startup. It also takes the fallible `create_with_seed`
  off the path: it used to be followed by `unwrap_or_default()`, which would have bought
  against `Pubkey::default()`. 83ns → 13ns, and one silent failure mode gone.
* **`bonding_curve_v2` has to be derived; there is nowhere to read it from.** Checked against
  captured mainnet `create_v2` transactions: it is not in the static keys, and it cannot be in
  the address lookup table either, because it is a PDA of a mint that transaction is creating.
  Everything else is taken from the data rather than computed — `bonding_curve` and
  `associated_bonding_curve` come out of the create instruction's accounts, `creator` out of
  its data, and `creator_vault` is cached per creator, which hits often because the same
  launchers launch repeatedly.
* **That derivation is 2.6µs and build flags do not move it.** Neither
  `-C target-cpu=native` nor `--cfg curve25519_dalek_backend="simd"` changes it, measured: the
  cost is a point decompression per candidate bump, which is a field exponentiation, and it is
  not what those backends accelerate. It is the largest single item left on the path and it is
  ~0.05% of the network hop that follows it.

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
