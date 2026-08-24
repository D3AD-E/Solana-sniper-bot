//! Hot path timing, measured rather than guessed.
//!
//! ```text
//! cargo test --release -p sniper bench_ -- --nocapture --test-threads=1
//! ```
//!
//! Lives inside the crate so it can drive the real `HotSniper::on_create` and the real
//! per-stage functions, with no test-only API widening.

#![cfg(test)]

use std::{
    collections::HashSet,
    sync::{atomic::AtomicBool, Arc},
    time::Instant,
};

use ed25519_dalek::{Signer, SigningKey};
use solana_sdk::pubkey::Pubkey;

use crate::{
    chain::{FeeRecipientCache, GlobalConfig, NoncePool},
    pumpfun::{CurveParams, PumpCreateInfo, TOKEN_2022_PROGRAM},
    sender::{Job, MAX_TX},
    template, HotSniper, Shared, SniperMetrics,
};

const ITERS: usize = 20_000;

fn percentiles(mut ns: Vec<u128>) -> (u128, u128, u128, u128) {
    ns.sort_unstable();
    let at = |p: f64| ns[((ns.len() as f64 * p) as usize).min(ns.len() - 1)];
    (ns[0], at(0.5), at(0.99), ns[ns.len() - 1])
}

fn report(label: &str, samples: Vec<u128>) {
    let (min, p50, p99, max) = percentiles(samples);
    println!("{label:<38} min {min:>7}ns  p50 {p50:>7}ns  p99 {p99:>8}ns  max {max:>9}ns");
}

/// Varied bytes: a byte-repeated key like `[0x44; 32]` contains the 8-byte u64 placeholder
/// patterns, which `find_unique` rightly rejects when building the template.
fn key(tag: u8) -> Pubkey {
    let mut k = [0u8; 32];
    for (i, b) in k.iter_mut().enumerate() {
        *b = tag.wrapping_mul(31).wrapping_add((i as u8).wrapping_mul(7)).wrapping_add(1);
    }
    Pubkey::new_from_array(k)
}

fn curve() -> CurveParams {
    CurveParams {
        initial_virtual_sol_reserves: 30_000_000_000,
        initial_virtual_token_reserves: 1_073_000_000_000_000,
        initial_real_token_reserves: 793_100_000_000_000,
        fee_basis_points: 95,
        creator_fee_basis_points: 5,
    }
}

fn info(n: usize) -> PumpCreateInfo {
    let mut mint = [0u8; 32];
    mint[..8].copy_from_slice(&(n as u64).to_le_bytes());
    mint[8] = 0xAA;
    let mut creator = [0u8; 32];
    creator[..8].copy_from_slice(&(n as u64 / 7).to_le_bytes()); // creators repeat, as on chain
    creator[8] = 0xBB;
    PumpCreateInfo {
        mint: Pubkey::new_from_array(mint),
        bonding_curve: key(0x11),
        associated_bonding_curve: key(0x22),
        creator: Pubkey::new_from_array(creator),
        user: key(0x33),
        token_program: TOKEN_2022_PROGRAM,
        dev_buy_lamports: 500_000_000,
        is_v2: true,
    }
}

/// Builds a HotSniper wired to `providers` drained channels, with no network or RPC.
///
/// `spinning` mirrors `sender_spin_micros`: a spinning receiver is picked up without a
/// kernel wake, a parked one costs the producer a futex wake per provider.
fn hot(providers: usize, spinning: bool) -> (HotSniper, Vec<std::thread::JoinHandle<()>>, Arc<AtomicBool>) {
    let buyer = key(0x44);
    let global = GlobalConfig {
        fee_recipient: key(0x55),
        buyback_fee_recipients: (0..8u8).map(|i| key(0x60 + i)).collect(),
        curve: curve(),
        fee_basis_points: 95,
        creator_fee_basis_points: 5,
    };

    let nonces = Arc::new(NoncePool::new(vec![key(0x77)]));
    // seed a nonce value without touching the network
    nonces
        .load_from_values(vec![[0x88; 32]])
        .expect("seed nonce");

    let whitelist = crate::whitelist::Whitelist::new(true);

    let stop = Arc::new(AtomicBool::new(false));
    let mut handles = Vec::new();
    let mut drains = Vec::new();
    for i in 0..providers {
        let (tx, rx) = crossbeam_channel::bounded::<Job>(64);
        let stop_rx = stop.clone();
        drains.push(std::thread::spawn(move || {
            if spinning {
                while !stop_rx.load(std::sync::atomic::Ordering::Relaxed) {
                    if rx.try_recv().is_err() {
                        std::hint::spin_loop();
                    }
                }
            } else {
                while rx.recv().is_ok() {}
            }
        }));
        handles.push(crate::sender::ProviderHandle {
            name: format!("p{i}"),
            tx,
            metrics: Arc::new(crate::sender::SenderMetrics::default()),
            tip_accounts: (0..8u8)
                .map(|j| {
                    let mut k = [0u8; 32];
                    k[0] = i as u8;
                    k[1] = j;
                    k
                })
                .collect(),
            tip_lamports: 2_000_000,
            cu_price: 6_000_000,
            endpoint_count: 6,
        });
    }

    let shared = Arc::new(Shared {
        whitelist,
        nonces,
        fee_recipients: FeeRecipientCache::new(&global),
        curve: curve(),
        metrics: Arc::new(SniperMetrics::default()),
        providers: handles,
        buy_lamports: 1_000_000_000,
        max_dev_buy_lamports: 2_900_000_000,
        haircut_bps: 30,
        slippage_bps: 100,
        buyer,
    });

    let static_accounts = template::StaticAccounts {
        fee_recipient: global.fee_recipient,
        buyback_fee_recipient: global.buyback_fee_recipients[0],
        user: buyer,
        user_volume_accumulator: crate::pumpfun::user_volume_accumulator(&buyer),
    };
    let tmpl = template::build(&static_accounts, 90_000);

    (
        HotSniper {
            shared,
            template: tmpl,
            seen: HashSet::with_capacity(1 << 16),
            job: Job::default(),
            seed_counter: 1,
            launch_counter: 0,
            vault_cache: Box::new([(Pubkey::default(), Pubkey::default()); 1024]),
        },
        drains,
        stop,
    )
}

/// Detection to queued, the whole thing, for a launch that actually fires.
#[test]
fn bench_on_create_end_to_end() {
    for spinning in [false, true] {
        for providers in [1usize, 6, 12] {
            let (mut sniper, drains, stop) = hot(providers, spinning);
            // warm the allocator and branch predictors
            for i in 0..1000 {
                sniper.on_create(&info(i));
            }
            let mut samples = Vec::with_capacity(ITERS);
            for i in 1000..1000 + ITERS {
                let launch = info(i);
                let t = Instant::now();
                let fired = sniper.on_create(&launch);
                samples.push(t.elapsed().as_nanos());
                assert!(fired.is_some());
            }
            let mode = if spinning { "spinning" } else { "parked  " };
            report(&format!("on_create {mode} {providers:>2} providers"), samples);
            stop.store(true, std::sync::atomic::Ordering::Relaxed);
            drop(sniper);
            for d in drains {
                let _ = d.join();
            }
        }
    }
}

/// Where the time inside `on_create` actually goes.
#[test]
fn bench_hot_path_stages() {
    let buyer = key(0x44);
    let c = curve();

    let mut s = Vec::with_capacity(ITERS);
    for i in 0..ITERS {
        let launch = info(i);
        let t = Instant::now();
        let v = crate::pumpfun::creator_vault(&launch.creator);
        s.push(t.elapsed().as_nanos());
        std::hint::black_box(v);
    }
    report("creator_vault (find_program_address)", s);

    let mut s = Vec::with_capacity(ITERS);
    for i in 0..ITERS {
        let launch = info(i);
        let t = Instant::now();
        let v = crate::pumpfun::bonding_curve_v2(&launch.mint);
        s.push(t.elapsed().as_nanos());
        std::hint::black_box(v);
    }
    report("bonding_curve_v2 (find_program_address)", s);

    let mut s = Vec::with_capacity(ITERS);
    for i in 0..ITERS {
        let seed = template::seed_bytes(i as u32);
        let seed = std::str::from_utf8(&seed).unwrap();
        let t = Instant::now();
        let v = Pubkey::create_with_seed(&buyer, seed, &TOKEN_2022_PROGRAM).unwrap();
        s.push(t.elapsed().as_nanos());
        std::hint::black_box(v);
    }
    report("create_with_seed (one sha256)", s);

    let mut s = Vec::with_capacity(ITERS);
    for _ in 0..ITERS {
        let t = Instant::now();
        let v = c.plan_buy(500_000_000, 1_000_000_000, 30, 100);
        s.push(t.elapsed().as_nanos());
        std::hint::black_box(v);
    }
    report("plan_buy (curve maths)", s);

    // the patching itself
    let static_accounts = template::StaticAccounts {
        fee_recipient: key(0x55),
        buyback_fee_recipient: key(0x66),
        user: buyer,
        user_volume_accumulator: crate::pumpfun::user_volume_accumulator(&buyer),
    };
    let mut tmpl = template::build(&static_accounts, 90_000);
    let patch_key = key(0x99).to_bytes();
    let mut s = Vec::with_capacity(ITERS);
    for _ in 0..ITERS {
        let o = tmpl.offsets;
        let t = Instant::now();
        tmpl.patch_key(o.mint, &patch_key);
        tmpl.patch_key(o.bonding_curve, &patch_key);
        tmpl.patch_key(o.associated_bonding_curve, &patch_key);
        tmpl.patch_key(o.bonding_curve_v2, &patch_key);
        tmpl.patch_key(o.creator_vault, &patch_key);
        tmpl.patch_key(o.token_account, &patch_key);
        tmpl.patch_token_program(&patch_key);
        tmpl.patch_key(o.nonce_account, &patch_key);
        tmpl.patch_key(o.nonce_value, &patch_key);
        tmpl.patch_key(o.fee_recipient, &patch_key);
        tmpl.patch_key(o.buyback_fee_recipient, &patch_key);
        tmpl.patch_seed(o.seed, b"012345");
        tmpl.patch_u64(o.amount, 1);
        tmpl.patch_u64(o.max_sol_cost, 2);
        s.push(t.elapsed().as_nanos());
    }
    report("patch 14 fields", s);

    let mut job = Job::default();
    let len = tmpl.tx.len();
    let mut s = Vec::with_capacity(ITERS);
    for _ in 0..ITERS {
        let t = Instant::now();
        job.len = len as u16;
        job.tx[..len].copy_from_slice(&tmpl.tx);
        s.push(t.elapsed().as_nanos());
    }
    report("copy tx into job buffer", s);
}

/// Isolates the cost of handing a job to a sender thread. A receiver blocked in `recv`
/// must be woken by the kernel, and that futex wake is charged to the *detect* thread.
#[test]
fn bench_channel_handoff() {
    let job = Job::default();

    // 1. no receiver at all: pure enqueue cost
    let (tx, rx) = crossbeam_channel::bounded::<Job>(ITERS + 16);
    let mut s = Vec::with_capacity(ITERS);
    for _ in 0..ITERS {
        let j = job.clone();
        let t = Instant::now();
        let _ = tx.try_send(j);
        s.push(t.elapsed().as_nanos());
    }
    report("try_send, no receiver", s);
    drop(rx);

    // 2. receiver parked in recv(): every send pays a wakeup
    let (tx, rx) = crossbeam_channel::bounded::<Job>(64);
    let h = std::thread::spawn(move || {
        while rx.recv().is_ok() {
            std::thread::sleep(std::time::Duration::from_micros(200));
        }
    });
    let mut s = Vec::with_capacity(2000);
    for _ in 0..2000 {
        let j = job.clone();
        let t = Instant::now();
        let _ = tx.try_send(j);
        s.push(t.elapsed().as_nanos());
        std::thread::sleep(std::time::Duration::from_micros(300));
    }
    report("try_send, receiver parked", s);
    drop(tx);
    let _ = h.join();

    // 3. receiver spinning on try_recv: no kernel involvement
    let (tx, rx) = crossbeam_channel::bounded::<Job>(64);
    let stop = Arc::new(AtomicBool::new(false));
    let stop2 = stop.clone();
    let h = std::thread::spawn(move || {
        while !stop2.load(std::sync::atomic::Ordering::Relaxed) {
            if rx.try_recv().is_err() {
                std::hint::spin_loop();
            }
        }
    });
    let mut s = Vec::with_capacity(2000);
    for _ in 0..2000 {
        let j = job.clone();
        let t = Instant::now();
        let _ = tx.try_send(j);
        s.push(t.elapsed().as_nanos());
        std::thread::sleep(std::time::Duration::from_micros(300));
    }
    report("try_send, receiver spinning", s);
    stop.store(true, std::sync::atomic::Ordering::Relaxed);
    let _ = h.join();
}

/// The write itself. A warm, connected, TCP_NODELAY socket to a local listener isolates the
/// syscall from the network, which is what the sender pays before bytes leave the box.
#[test]
fn bench_socket_write() {
    use std::io::Write;
    use std::net::{TcpListener, TcpStream};

    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();
    let reader = std::thread::spawn(move || {
        let (mut sock, _) = listener.accept().unwrap();
        let mut buf = [0u8; 8192];
        loop {
            match std::io::Read::read(&mut sock, &mut buf) {
                Ok(0) | Err(_) => break,
                Ok(_) => {}
            }
        }
    });

    let mut sock = TcpStream::connect(addr).unwrap();
    sock.set_nodelay(true).unwrap();
    // a full request: head + json wrapped base64 transaction
    let request = vec![0x41u8; 1500];

    for _ in 0..1000 {
        sock.write_all(&request).unwrap();
    }
    let mut s = Vec::with_capacity(ITERS);
    for _ in 0..ITERS {
        let t = Instant::now();
        sock.write_all(&request).unwrap();
        s.push(t.elapsed().as_nanos());
    }
    report("socket write, 1500 bytes", s);
    drop(sock);
    let _ = reader.join();
}

/// What the sender threads do, off the detect path but still worth knowing.
#[test]
fn bench_sender_stages() {
    use base64::{engine::general_purpose::STANDARD, Engine};

    let signing = SigningKey::from_bytes(&[7u8; 32]);
    let msg = vec![0x5Au8; 900];
    let mut s = Vec::with_capacity(ITERS);
    for _ in 0..ITERS {
        let t = Instant::now();
        let sig = signing.sign(&msg);
        s.push(t.elapsed().as_nanos());
        std::hint::black_box(sig);
    }
    report("ed25519 sign", s);

    let tx = vec![0x5Au8; 1043];
    let mut b64 = vec![0u8; MAX_TX * 4 / 3 + 8];
    let mut s = Vec::with_capacity(ITERS);
    for _ in 0..ITERS {
        let t = Instant::now();
        let n = STANDARD.encode_slice(&tx, &mut b64).unwrap();
        s.push(t.elapsed().as_nanos());
        std::hint::black_box(n);
    }
    report("base64 encode 1043 bytes", s);
}
