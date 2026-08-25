//! In-process pump.fun sniper.
//!
//! Detection happens inside the deshred thread: `HotSniper::on_create` is handed the
//! already-parsed transaction, patches the prebuilt buy template, and hands one job per
//! provider to a sender thread over a bounded channel. There is no cross-process hop, no
//! base64 on the detect thread, no serialization and no logging.
//!
//! Everything the hot path reads is either owned by the calling thread or an `ArcSwap`
//! updated in the background: whitelist, nonce values, fee recipients.

mod bench;
pub mod batch_write;
pub mod chain;
pub mod config;
pub mod providers;
pub mod position;
pub mod pumpfun;
pub mod sender;
pub mod template;
pub mod whitelist;
pub mod wire;

use std::{
    collections::HashSet,
    path::Path,
    sync::{
        atomic::{AtomicBool, AtomicU64, Ordering},
        Arc,
    },
    thread::JoinHandle,
    time::{SystemTime, UNIX_EPOCH},
};

use ed25519_dalek::SigningKey;
use log::{info, warn};
use solana_sdk::pubkey::Pubkey;

use crate::{
    chain::{FeeRecipientCache, NoncePool},
    position::{ModeConfig, OpenPosition, PositionGate},
    config::SniperConfig,
    pumpfun::{CurveParams, PumpCreateInfo},
    sender::{Job, ProviderHandle, MAX_TX},
    template::{seed_bytes, Template, SEED_LEN},
    whitelist::Whitelist,
};

#[derive(Default, Debug)]
pub struct SniperMetrics {
    /// pump.fun creates decoded from shreds
    pub creates_seen: AtomicU64,
    /// creates whose launcher is whitelisted
    pub creates_whitelisted: AtomicU64,
    /// creates skipped because the mint was already handled
    pub duplicates: AtomicU64,
    /// creates skipped because the dev buy was too large
    pub skipped_dev_buy: AtomicU64,
    /// creates skipped because no nonce was available
    pub skipped_no_nonce: AtomicU64,
    /// launches that produced at least one job
    pub fired: AtomicU64,
    /// individual provider jobs queued
    pub jobs_queued: AtomicU64,
}

/// Shared, background-updated state. Safe to read from the hot path.
pub struct Shared {
    pub whitelist: Whitelist,
    pub nonces: Arc<NoncePool>,
    pub fee_recipients: FeeRecipientCache,
    pub curve: CurveParams,
    pub metrics: Arc<SniperMetrics>,
    pub providers: Vec<ProviderHandle>,
    pub buy_lamports: u64,
    pub max_dev_buy_lamports: u64,
    pub haircut_bps: u64,
    pub slippage_bps: u64,
    pub buyer: Pubkey,
    /// sync / test / ghost gating
    pub gate: Arc<PositionGate>,
    pub ghost_mode: bool,
}

pub struct Sniper {
    pub shared: Arc<Shared>,
    template: Template,
    threads: Vec<JoinHandle<()>>,
}

impl Sniper {
    /// Reads config, validates the nonce accounts, fetches the current pump.fun global
    /// account, builds the template and starts every background thread. Fails loudly: a
    /// half-configured sniper is worse than none.
    pub fn start(config_path: &Path, exit: Arc<AtomicBool>) -> Result<Self, String> {
        let cfg = SniperConfig::load(config_path)?;
        let signing_key = read_keypair(&cfg.keypair_path)?;
        let buyer = Pubkey::new_from_array(signing_key.verifying_key().to_bytes());

        let global = chain::fetch_global(&cfg.rpc_url)?;
        info!(
            "sniper: buyer {buyer}, fee recipient {}, buying {} lamports at {} bps fee",
            global.fee_recipient,
            cfg.buy_lamports,
            global.fee_basis_points + global.creator_fee_basis_points
        );

        if cfg.nonce_accounts.is_empty() {
            return Err("no nonce_accounts configured: the buy template advances a durable \
                        nonce, so at least one is required"
                .to_string());
        }
        let nonce_keys = cfg
            .nonce_accounts
            .iter()
            .map(|s| {
                s.parse::<Pubkey>()
                    .map_err(|e| format!("bad nonce account {s}: {e}"))
            })
            .collect::<Result<Vec<_>, _>>()?;
        let nonces = Arc::new(NoncePool::new(nonce_keys));
        nonces.load_once(&cfg.rpc_url, &buyer)?;
        info!("sniper: {} nonce accounts ready", nonces.len());

        let static_accounts = template::StaticAccounts {
            fee_recipient: global.fee_recipient,
            buyback_fee_recipient: *global
                .buyback_fee_recipients
                .first()
                .ok_or("global account has no buyback fee recipients")?,
            user: buyer,
            user_volume_accumulator: pumpfun::user_volume_accumulator(&buyer),
        };

        let mut threads = Vec::new();
        let mut providers = Vec::new();
        for p in cfg.providers.iter().filter(|p| p.enabled) {
            let (handle, join) = sender::spawn(
                p.clone(),
                signing_key.clone(),
                cfg.dry_run,
                64,
                cfg.sender_spin_micros,
            )?;
            info!(
                "sniper: provider {} -> {}:{}{} tip {} across {} accounts, cu price {}",
                p.name,
                p.host,
                p.port,
                p.path,
                p.tip_lamports,
                handle.tip_accounts.len(),
                p.cu_price
            );
            providers.push(handle);
            threads.push(join);
        }
        if providers.is_empty() {
            return Err("no enabled providers".to_string());
        }

        let whitelist = Whitelist::new(cfg.whitelist_disabled);
        if !cfg.whitelist_path.is_empty() {
            threads.push(whitelist.spawn_refresher(cfg.whitelist_path.clone().into(), exit.clone()));
        } else if !cfg.whitelist_disabled {
            warn!("sniper: no whitelist_path set and whitelist not disabled, nothing will fire");
        }

        threads.push(nonces.spawn_refresher(cfg.rpc_url.clone(), cfg.nonce_refresh_ms, exit.clone()));

        let fee_recipients = FeeRecipientCache::new(&global);
        threads.push(fee_recipients.spawn_refresher(cfg.rpc_url.clone(), 30, exit.clone()));

        let modes = ModeConfig {
            sync_mode: cfg.sync_mode,
            test_mode: cfg.test_mode,
            ghost_mode: cfg.ghost_mode,
            hold_ms: cfg.hold_ms,
            poll_ms: cfg.position_poll_ms,
            buy_timeout_ms: cfg.buy_timeout_ms,
        };
        info!(
            "sniper: sync_mode {} test_mode {} ghost_mode {}",
            modes.sync_mode, modes.test_mode, modes.ghost_mode
        );
        if modes.ghost_mode {
            info!("sniper: ghost mode, nothing will be sent to a provider");
        }
        let (gate, gate_thread) =
            position::start(cfg.rpc_url.clone(), modes, global.curve, exit.clone());
        threads.push(gate_thread);

        let shared = Arc::new(Shared {
            whitelist,
            nonces,
            fee_recipients,
            curve: global.curve,
            metrics: Arc::new(SniperMetrics::default()),
            providers,
            buy_lamports: cfg.buy_lamports,
            max_dev_buy_lamports: cfg.max_dev_buy_lamports,
            haircut_bps: cfg.haircut_bps,
            slippage_bps: cfg.slippage_bps,
            buyer,
            gate,
            ghost_mode: cfg.ghost_mode,
        });

        let template = template::build(&static_accounts, cfg.cu_limit);
        info!("sniper: template ready\n{}", template.describe());

        Ok(Self {
            shared,
            template,
            threads,
        })
    }

    /// Moves the per-thread hot state out. Call once, from the deshred thread.
    pub fn into_hot(self) -> (HotSniper, Vec<JoinHandle<()>>) {
        let Sniper {
            shared,
            template,
            threads,
        } = self;

        // start the seed counter somewhere unpredictable, so a restart cannot collide with a
        // token account a previous run created and has not sold yet
        let seed_counter = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| (d.subsec_nanos() ^ d.as_secs() as u32) % 1_000_000)
            .unwrap_or(0);

        (
            HotSniper {
                shared,
                template,
                seen: HashSet::with_capacity(1 << 16),
                job: Job::default(),
                seed_counter,
                launch_counter: 0,
                vault_cache: Box::new(
                    [(Pubkey::default(), Pubkey::default()); VAULT_CACHE_SLOTS],
                ),
            },
            threads,
        )
    }
}

/// Per-thread hot state. Exactly one thread owns it.
pub struct HotSniper {
    pub shared: Arc<Shared>,
    template: Template,
    seen: HashSet<Pubkey>,
    job: Job,
    seed_counter: u32,
    launch_counter: usize,
    /// Direct-mapped cache of creator -> creator vault.
    ///
    /// `find_program_address` costs ~2.7us because every candidate bump has to be checked
    /// against the ed25519 curve, and the same creators launch token after token, so the
    /// hit rate is high. A wrong entry is impossible: the creator is compared in full.
    vault_cache: Box<[(Pubkey, Pubkey); VAULT_CACHE_SLOTS]>,
}

const VAULT_CACHE_SLOTS: usize = 1024;

#[inline(always)]
fn vault_slot(creator: &Pubkey) -> usize {
    let b = creator.as_ref();
    (u16::from_le_bytes([b[0], b[1]]) as usize) % VAULT_CACHE_SLOTS
}

/// What a fired launch used. Handed to the non-hot side for bookkeeping and for selling:
/// the token account is derived from a seed, so the seller needs the seed to find it.
#[derive(Debug, Clone, Copy)]
pub struct FiredLaunch {
    pub mint: Pubkey,
    pub token_account: Pubkey,
    pub seed: [u8; SEED_LEN],
    pub amount: u64,
    pub max_sol_cost: u64,
}

impl HotSniper {
    pub fn metrics(&self) -> &SniperMetrics {
        &self.shared.metrics
    }

    /// Hot path. Returns what was fired, or None.
    ///
    /// No allocation, no `String`/`format!`, no logging, no syscalls.
    #[inline]
    pub fn on_create(&mut self, info: &PumpCreateInfo, slot: u64) -> Option<FiredLaunch> {
        let m = &self.shared.metrics;
        m.creates_seen.fetch_add(1, Ordering::Relaxed);

        if !self.shared.whitelist.contains(&info.user) {
            return None;
        }
        m.creates_whitelisted.fetch_add(1, Ordering::Relaxed);

        // one token at a time, or stopped after a test round trip
        if !self.shared.gate.may_fire() {
            return None;
        }

        if !self.seen.insert(info.mint) {
            m.duplicates.fetch_add(1, Ordering::Relaxed);
            return None;
        }
        if self.seen.len() > 50_000 {
            self.seen.clear();
        }

        if self.shared.max_dev_buy_lamports > 0
            && info.dev_buy_lamports >= self.shared.max_dev_buy_lamports
        {
            m.skipped_dev_buy.fetch_add(1, Ordering::Relaxed);
            return None;
        }

        // every provider variant of this launch shares one nonce, so only one can land
        let Some((nonce_account, nonce_value)) = self.shared.nonces.take() else {
            m.skipped_no_nonce.fetch_add(1, Ordering::Relaxed);
            return None;
        };

        let plan = self.shared.curve.plan_buy(
            info.dev_buy_lamports,
            self.shared.buy_lamports,
            self.shared.haircut_bps,
            self.shared.slippage_bps,
        );
        if plan.amount == 0 {
            return None;
        }

        // the token account comes from a seed rather than the ATA program: one sha256 here,
        // and ~18k fewer compute units on chain
        self.seed_counter = (self.seed_counter + 1) % 1_000_000;
        let seed = seed_bytes(self.seed_counter);
        // safety: `seed_bytes` only ever produces ASCII digits
        let seed_str = unsafe { core::str::from_utf8_unchecked(&seed) };
        let token_account =
            Pubkey::create_with_seed(&self.shared.buyer, seed_str, &info.token_program)
                .unwrap_or_default();

        let cache_slot = vault_slot(&info.creator);
        let creator_vault = if self.vault_cache[cache_slot].0 == info.creator {
            self.vault_cache[cache_slot].1
        } else {
            let v = pumpfun::creator_vault(&info.creator);
            self.vault_cache[cache_slot] = (info.creator, v);
            v
        };
        let bonding_curve_v2 = pumpfun::bonding_curve_v2(&info.mint);

        self.launch_counter = self.launch_counter.wrapping_add(1);
        let fee_recipient = self.shared.fee_recipients.fee_recipient();
        let buyback = self.shared.fee_recipients.buyback(self.launch_counter);

        let t = &mut self.template;
        let o = t.offsets;
        t.patch_key(o.fee_recipient, &fee_recipient);
        t.patch_key(o.buyback_fee_recipient, &buyback);
        t.patch_key(o.mint, &info.mint.to_bytes());
        t.patch_key(o.bonding_curve, &info.bonding_curve.to_bytes());
        t.patch_key(
            o.associated_bonding_curve,
            &info.associated_bonding_curve.to_bytes(),
        );
        t.patch_key(o.bonding_curve_v2, &bonding_curve_v2.to_bytes());
        t.patch_key(o.creator_vault, &creator_vault.to_bytes());
        t.patch_key(o.token_account, &token_account.to_bytes());
        t.patch_token_program(&info.token_program.to_bytes());
        t.patch_seed(o.seed, &seed);
        t.patch_u64(o.amount, plan.amount);
        t.patch_u64(o.max_sol_cost, plan.max_sol_cost);
        t.patch_key(o.nonce_account, &nonce_account);
        t.patch_key(o.nonce_value, &nonce_value);

        let len = t.tx.len();
        if len > MAX_TX {
            return None;
        }

        if !self.shared.gate.claim() {
            return None;
        }

        // ghost mode stops here: the position is recorded and priced out on paper, and
        // nothing is handed to a provider
        if self.shared.ghost_mode {
            self.shared.gate.opened(OpenPosition {
                mint: info.mint,
                create_slot: slot,
                bonding_curve: info.bonding_curve,
                token_account,
                amount: plan.amount,
                cost: self.shared.buy_lamports,
                ghost: true,
            });
            m.fired.fetch_add(1, Ordering::Relaxed);
            return Some(FiredLaunch {
                mint: info.mint,
                token_account,
                seed,
                amount: plan.amount,
                max_sol_cost: plan.max_sol_cost,
            });
        }

        let mut queued = 0u64;
        for i in 0..self.shared.providers.len() {
            let (tip_account, tip_lamports, cu_price) = {
                let p = &self.shared.providers[i];
                let tip = p.tip_accounts[self.launch_counter % p.tip_accounts.len()];
                (tip, p.tip_lamports, p.cu_price)
            };
            let t = &mut self.template;
            let o = t.offsets;
            t.patch_key(o.tip_account, &tip_account);
            t.patch_u64(o.tip_lamports, tip_lamports);
            t.patch_u64(o.cu_price, cu_price);

            self.job.len = len as u16;
            self.job.tx[..len].copy_from_slice(&t.tx);
            self.shared.providers[i].try_send(self.job.clone());
            queued += 1;
        }

        m.jobs_queued.fetch_add(queued, Ordering::Relaxed);
        if queued == 0 {
            return None;
        }
        m.fired.fetch_add(1, Ordering::Relaxed);
        self.shared.gate.opened(OpenPosition {
            mint: info.mint,
            create_slot: slot,
            bonding_curve: info.bonding_curve,
            token_account,
            amount: plan.amount,
            cost: plan.max_sol_cost,
            ghost: false,
        });

        Some(FiredLaunch {
            mint: info.mint,
            token_account,
            seed,
            amount: plan.amount,
            max_sol_cost: plan.max_sol_cost,
        })
    }
}

/// Reads a solana-cli keypair json file (a 64 byte array).
fn read_keypair(path: &str) -> Result<SigningKey, String> {
    let raw = std::fs::read_to_string(path).map_err(|e| format!("read keypair {path}: {e}"))?;
    let bytes: Vec<u8> =
        serde_json::from_str(&raw).map_err(|e| format!("parse keypair {path}: {e}"))?;
    if bytes.len() < 32 {
        return Err(format!("keypair {path} is shorter than 32 bytes"));
    }
    let seed: [u8; 32] = bytes[..32].try_into().unwrap();
    Ok(SigningKey::from_bytes(&seed))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keypair_file_round_trips() {
        let dir = std::env::temp_dir().join("sniper_keypair_test");
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("kp.json");
        let secret = [3u8; 32];
        let key = SigningKey::from_bytes(&secret);
        let mut full = secret.to_vec();
        full.extend_from_slice(&key.verifying_key().to_bytes());
        std::fs::write(&path, serde_json::to_string(&full).unwrap()).unwrap();

        let loaded = read_keypair(path.to_str().unwrap()).unwrap();
        assert_eq!(loaded.to_bytes(), secret);
    }
}
