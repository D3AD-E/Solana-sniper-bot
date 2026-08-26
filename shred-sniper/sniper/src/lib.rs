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
pub mod confirm;
pub mod providers;
pub mod position;
pub mod pumpfun;
pub mod sender;
pub mod template;
pub mod tip;
pub mod whitelist;
pub mod wire;

use std::{
    path::Path,
    sync::{
        atomic::{AtomicBool, AtomicU64, Ordering},
        Arc,
    },
    thread::JoinHandle,
    time::{SystemTime, UNIX_EPOCH},
};

use ahash::AHashSet;
use ed25519_dalek::SigningKey;
use log::{info, warn};
use solana_sdk::pubkey::Pubkey;

use crate::{
    chain::{FeeRecipientCache, NoncePool},
    position::{ModeConfig, OpenPosition, PositionGate},
    config::SniperConfig,
    confirm::{LaunchWatch, Pass, Session},
    pumpfun::{BuyPlan, CurveParams, PumpBuyInfo, PumpCreateInfo},
    sender::{Job, ProviderHandle, TipOffsets, TxBuf, MAX_TX},
    template::{SeedTable, Template, SEED_LEN},
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
    /// confirm mode: creates from a fresh deployer, registered as pending
    pub confirm_pending: AtomicU64,
    /// confirm mode: creates skipped because the deployer was not fresh
    pub confirm_not_fresh: AtomicU64,
    /// confirm mode: launches skipped because an elite operator landed before the trigger
    pub confirm_elite_ahead: AtomicU64,
    /// confirm mode: launches skipped because the curve was at/over the completion cap
    pub confirm_curve_capped: AtomicU64,
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
    /// use pump `buy_exact_sol_in` (fix SOL, floor tokens) instead of `buy` (fix tokens, cap
    /// SOL) - the leader's instruction, and it removes the exact-token depth-estimate revert.
    pub buy_exact_sol_in: bool,
    /// token accounts for every seed, derived at startup
    pub seed_table: SeedTable,
    /// v1.1 confirmation-trigger params, or None when the whitelist path is in use.
    pub confirm: Option<confirm::Params>,
    /// dynamic Jito tip params, or None to use each provider's static tip.
    pub tip: Option<tip::TipParams>,
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
        warn_if_built_without_simd();
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

        // the leader uses buy_exact_sol_in on ~75% of buys; default it on. Same accounts as
        // buy, so the template only swaps the discriminator + the meaning of the two args.
        let buy_exact_sol_in = std::env::var("SNIPER_BUY_EXACT_SOL_IN")
            .map(|v| v.trim() != "0")
            .unwrap_or(true);
        info!("sniper: buy instruction = {}",
            if buy_exact_sol_in { "buy_exact_sol_in" } else { "buy" });

        // built before the senders start: each one needs the offsets of the three fields it
        // owns, so it can stamp them onto the shared body on its own thread
        let template = template::build(&static_accounts, cfg.cu_limit, buy_exact_sol_in);
        info!("sniper: template ready\n{}", template.describe());
        let tip_offsets = TipOffsets {
            tip_account: template.offsets.tip_account,
            tip_lamports: template.offsets.tip_lamports,
            cu_price: template.offsets.cu_price,
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
                tip_offsets,
                template.tx.len(),
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

        // start the seed counter somewhere unpredictable, so a restart cannot collide with a
        // token account a previous run created and has not sold yet
        let seed_start = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| (d.subsec_nanos() ^ d.as_secs() as u32) % 1_000_000)
            .unwrap_or(0);
        let seed_table = SeedTable::build(&buyer, seed_start, SEED_TABLE_LEN)?;
        info!("sniper: {} token accounts derived", seed_table.len());

        // v1.1 confirmation-trigger tables. When SNIPER_CONFIRM_MODE=1 these replace the
        // whitelist for the MAIN book; the reloader keeps them current under a running proxy.
        let confirm = if confirm::Params::enabled() {
            let dev_path = std::env::var("SNIPER_DEV_HISTORY")
                .unwrap_or_else(|_| "dev_history.txt".into());
            let watch_path = std::env::var("SNIPER_WATCH_WALLETS")
                .unwrap_or_else(|_| "watch_wallets.tsv".into());
            match (
                confirm::load_devs_from_file(&dev_path),
                confirm::load_watch_from_file(&watch_path),
            ) {
                (Ok((nd, _)), Ok((nw, _))) => {
                    info!("sniper: confirm mode ON, {nd} known devs, {nw} watch wallets");
                }
                (d, w) => {
                    return Err(format!(
                        "confirm mode on but tables failed to load: devs={d:?} watch={w:?} \
                         (SNIPER_DEV_HISTORY, SNIPER_WATCH_WALLETS)"
                    ));
                }
            }
            confirm::spawn_reloader(dev_path, watch_path, std::time::Duration::from_secs(60));
            confirm::CONFIRM_MODE.store(true, std::sync::atomic::Ordering::Relaxed);
            Some(confirm::Params::from_env())
        } else {
            None
        };

        // dynamic Jito tip. Fallback is the largest configured provider tip, so a tip is
        // never below what the config already pays if the stream has not published yet.
        let static_tip = cfg
            .providers
            .iter()
            .filter(|p| p.enabled)
            .map(|p| p.tip_lamports)
            .max()
            .unwrap_or(2_000_000);
        let tip_params = tip::TipParams::from_env(static_tip);
        if let Some(tp) = tip_params {
            let url = std::env::var("SNIPER_JITO_TIP_FLOOR_URL")
                .unwrap_or_else(|_| "https://bundles.jito.wtf/api/v1/bundles/tip_floor".into());
            info!(
                "sniper: dynamic tip ON (p{} floor, {} bps of size, +{} bps/buy, {}..{} lamports)",
                tp.pctile, tp.size_bps, tp.comp_step_bps, tp.min_lamports, tp.max_lamports
            );
            tip::spawn_poller(url, std::time::Duration::from_secs(2));
        }

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
            buy_exact_sol_in,
            seed_table,
            confirm,
            tip: tip_params,
        });

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

        (
            HotSniper {
                shared,
                template,
                seen: AHashSet::with_capacity(1 << 16),
                tx_ring: (0..TX_RING).map(|_| Arc::new(TxBuf::default())).collect(),
                ring_cursor: 0,
                seed_index: 0,
                launch_counter: 0,
                vault_cache: Box::new(
                    [(Pubkey::default(), Pubkey::default()); VAULT_CACHE_SLOTS],
                ),
                session: Session::default(),
                pending: ahash::AHashMap::with_capacity(256),
                cur_slot: 0,
                seen_buys: AHashSet::with_capacity(1024),
            },
            threads,
        )
    }
}

/// Per-thread hot state. Exactly one thread owns it.
pub struct HotSniper {
    pub shared: Arc<Shared>,
    template: Template,
    /// Mints already fired on. `ahash`: this is a 32 byte key looked up on the hot path.
    seen: AHashSet<Pubkey>,
    /// Preallocated transaction bodies handed to the senders.
    ///
    /// Every provider variant of a launch shares one body, so the fan-out copies 1232 bytes
    /// once instead of once per provider. A slot is reused as soon as every sender has
    /// finished with the previous launch that used it, which is the normal case; when one is
    /// still in flight the launch allocates a fresh body rather than making the hot path
    /// wait for it.
    tx_ring: Vec<Arc<TxBuf>>,
    ring_cursor: usize,
    /// cursor into `Shared::seed_table`
    seed_index: usize,
    launch_counter: usize,
    /// Direct-mapped cache of creator -> creator vault.
    ///
    /// `find_program_address` costs ~2.7us because every candidate bump has to be checked
    /// against the ed25519 curve, and the same creators launch token after token, so the
    /// hit rate is high. A wrong entry is impossible: the creator is compared in full.
    vault_cache: Box<[(Pubkey, Pubkey); VAULT_CACHE_SLOTS]>,
    /// v1.1 confirm mode: deployers seen this session (freshness), the create blocks being
    /// watched, and the slot they belong to. All empty/unused when confirm is off.
    session: Session,
    pending: ahash::AHashMap<Pubkey, PendingLaunch>,
    cur_slot: u64,
    /// confirming-buy signatures already counted this slot. The early-detect path re-emits
    /// the same buy on every later shred of the segment, so without this dedup one buy is
    /// counted many times and the trigger fires off a single buy. Cleared on slot advance.
    seen_buys: AHashSet<[u8; 8]>,
}

/// A launch whose create block is being watched for the confirmation trigger.
struct PendingLaunch {
    info: PumpCreateInfo,
    slot: u64,
    watch: LaunchWatch,
}

const VAULT_CACHE_SLOTS: usize = 1024;

/// How many token accounts are derived up front. At ~400ns each this is ~26ms of startup and
/// ~4.6MB, and it covers more launches than a process is going to see between restarts.
const SEED_TABLE_LEN: usize = 1 << 16;

/// How many launch bodies are kept preallocated. Launches are seconds apart and a sender
/// lets go of a body as soon as it has copied it, so one slot would almost always do; the
/// ring is here so a wedged provider cannot force an allocation on every later launch.
const TX_RING: usize = 8;

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
    /// raw instruction arg0: exact tokens under `buy`, sol_in under `buy_exact_sol_in`
    pub amount: u64,
    /// raw instruction arg1: sol cap under `buy`, min-tokens floor under `buy_exact_sol_in`
    pub max_sol_cost: u64,
    /// Instruction-INDEPENDENT cost basis, lamports (the priced budget). The seller's
    /// stop-loss and all PnL account against this — never against `max_sol_cost`, whose
    /// meaning flips with the instruction.
    pub cost_lamports: u64,
    /// Instruction-INDEPENDENT expected token fill at the priced depth.
    pub expected_tokens: u64,
    // carried so the seller can be handed a fill without the original create in hand -- the
    // confirmation trigger fires from a buy, where the forwarder no longer has the create.
    pub bonding_curve: Pubkey,
    pub associated_bonding_curve: Pubkey,
    pub creator: Pubkey,
    pub token_program: Pubkey,
}

impl HotSniper {
    pub fn metrics(&self) -> &SniperMetrics {
        &self.shared.metrics
    }

    /// Copies the patched template into a shareable body, reusing a ring slot when every
    /// sender has already let go of it.
    ///
    /// Takes its fields rather than `&mut self` so the caller can keep its borrow of
    /// `shared.metrics` alive across the call.
    #[inline]
    fn fill_body(ring: &mut [Arc<TxBuf>], cursor: &mut usize, template: &[u8]) -> Arc<TxBuf> {
        let len = template.len();
        let idx = *cursor;
        *cursor += 1;
        if *cursor == ring.len() {
            *cursor = 0;
        }
        let slot = &mut ring[idx];
        if let Some(buf) = Arc::get_mut(slot) {
            buf.len = len as u16;
            buf.tx[..len].copy_from_slice(template);
            return slot.clone();
        }
        // a sender is still holding this slot. Allocating is cheaper than waiting for it.
        let mut buf = TxBuf::default();
        buf.len = len as u16;
        buf.tx[..len].copy_from_slice(template);
        let fresh = Arc::new(buf);
        *slot = fresh.clone();
        fresh
    }

    /// Hot path. A pump create was decoded; decide, and fire immediately on the whitelist
    /// path or register the launch for the confirmation trigger.
    ///
    /// No allocation, no `String`/`format!`, no logging, no syscalls.
    #[inline]
    pub fn on_create(&mut self, info: &PumpCreateInfo, slot: u64) -> Option<FiredLaunch> {
        // never bind `&self.shared.metrics` across a `&mut self` call (confirm_register /
        // fire_launch below); the atomics are incremented inline instead.
        self.shared.metrics.creates_seen.fetch_add(1, Ordering::Relaxed);

        // v1.1 confirmation trigger: do not fire on the create. Register a fresh deployer's
        // launch and wait for the create block to confirm demand (see on_buy).
        if let Some(params) = self.shared.confirm.clone() {
            self.confirm_register(info, slot, &params);
            return None;
        }

        if !self.shared.whitelist.contains(&info.user) {
            return None;
        }
        self.shared.metrics.creates_whitelisted.fetch_add(1, Ordering::Relaxed);

        if !self.seen.insert(info.mint) {
            self.shared.metrics.duplicates.fetch_add(1, Ordering::Relaxed);
            return None;
        }
        if self.seen.len() > 50_000 {
            self.seen.clear();
        }

        if self.shared.max_dev_buy_lamports > 0
            && info.dev_buy_lamports >= self.shared.max_dev_buy_lamports
        {
            self.shared.metrics.skipped_dev_buy.fetch_add(1, Ordering::Relaxed);
            return None;
        }

        let plan = if self.shared.buy_exact_sol_in {
            self.shared.curve.plan_exact_sol_in(
                info.dev_buy_lamports,
                self.shared.buy_lamports,
                self.shared.slippage_bps,
            )
        } else {
            self.shared.curve.plan_buy(
                info.dev_buy_lamports,
                self.shared.buy_lamports,
                self.shared.haircut_bps,
                self.shared.slippage_bps,
            )
        };
        if plan.amount == 0 {
            return None;
        }
        // whitelist path has no create-block confirmation count; competition = 0.
        self.fire_launch(info, slot, plan, 0)
    }

    /// v1.1 confirm mode: register a fresh deployer's launch so its create block can be
    /// watched. Dedups the create and drops pending launches from sealed (earlier) slots.
    #[inline]
    fn confirm_register(&mut self, info: &PumpCreateInfo, slot: u64, _params: &confirm::Params) {
        // never bind `&self.shared.metrics` across a `&mut self` call below.
        self.seal_old_slots(slot);
        if !self.seen.insert(info.mint) {
            self.shared.metrics.duplicates.fetch_add(1, Ordering::Relaxed);
            return;
        }
        if self.seen.len() > 50_000 {
            self.seen.clear();
        }
        if !self.session.dev_is_fresh(&info.creator) {
            self.shared.metrics.confirm_not_fresh.fetch_add(1, Ordering::Relaxed);
            return;
        }
        if self.pending.len() > 4096 {
            self.pending.retain(|_, p| p.slot >= slot);
        }
        self.shared.metrics.confirm_pending.fetch_add(1, Ordering::Relaxed);
        self.pending.insert(
            info.mint,
            PendingLaunch { info: *info, slot, watch: LaunchWatch::new(info.dev_buy_lamports) },
        );
    }

    /// v1.1 confirm mode: a pump buy was decoded. Advance the matching create block's watcher
    /// and fire when the trigger is met. Returns what was fired, or None.
    #[inline]
    pub fn on_buy(&mut self, buy: &PumpBuyInfo, slot: u64) -> Option<FiredLaunch> {
        let Some(params) = self.shared.confirm.clone() else {
            return None;
        };
        self.seal_old_slots(slot);
        // fire only on confirming buys IN the create block; buys in a later slot are too late.
        // Check pending FIRST: segments complete out of order, so a buy can arrive before its
        // create registers. If we deduped before that check, the buy would burn its dedup slot
        // with no pending launch, and the later (in-order) re-emit would be dropped as a dup -
        // silently undercounting n_conf. Dedup only once we have a launch to count it against.
        let (result, info) = {
            let Some(p) = self.pending.get_mut(&buy.mint) else {
                return None;
            };
            if p.slot != slot {
                return None;
            }
            // the early-detect path re-emits the same buy on every later shred of the segment;
            // count each confirming buy exactly once. seen_buys is a disjoint field from pending.
            if !self.seen_buys.insert(buy.sig8) {
                return None;
            }
            (p.watch.on_buy(&buy.buyer, buy.sol_lamports, &params), p.info)
        };
        // note: never bind `&self.shared.metrics` across the `&mut self` fire below.
        match result {
            Ok(fire) => {
                self.pending.remove(&buy.mint);
                // price against the FULL curve depth ahead of us (dev + confirmers). With
                // buy_exact_sol_in this only sets the min-tokens floor; with buy it sets the
                // exact token amount (whose depth error is what used to blow max_sol_cost).
                let plan = if self.shared.buy_exact_sol_in {
                    self.shared.curve.plan_exact_sol_in(
                        fire.prior_flow_lamports,
                        fire.budget_lamports,
                        self.shared.slippage_bps,
                    )
                } else {
                    self.shared.curve.plan_buy(
                        fire.prior_flow_lamports,
                        fire.budget_lamports,
                        self.shared.haircut_bps,
                        self.shared.slippage_bps,
                    )
                };
                if plan.amount == 0 {
                    return None;
                }
                let _ = fire.book; // Book::{Main,FollowSymbiont,FollowWhale} — same fire path
                self.fire_launch(&info, slot, plan, fire.n_conf)
            }
            Err(Pass::EliteAhead) => {
                self.shared.metrics.confirm_elite_ahead.fetch_add(1, Ordering::Relaxed);
                self.pending.remove(&buy.mint);
                None
            }
            Err(Pass::CurveCapped) => {
                self.shared.metrics.confirm_curve_capped.fetch_add(1, Ordering::Relaxed);
                self.pending.remove(&buy.mint);
                None
            }
            Err(Pass::Dead) => {
                self.pending.remove(&buy.mint);
                None
            }
            Err(Pass::Watching) => None,
        }
    }

    /// The create block for a launch lives in one slot; once the proxy moves to a later slot,
    /// no more confirming buys can land, so drop everything still pending from earlier slots.
    #[inline]
    fn seal_old_slots(&mut self, slot: u64) {
        if slot > self.cur_slot {
            if !self.pending.is_empty() {
                // one slot of grace: segments complete out of order, so a buy from slot N+1 can
                // be reconstructed before slot N's create block is done. Dropping only launches
                // two-plus slots old keeps the current create block alive through that reordering.
                self.pending.retain(|_, p| p.slot + 1 >= slot);
            }
            // confirming-buy dedup only matters within a create slot; reset periodically so the
            // set cannot grow without bound.
            if self.seen_buys.len() > 4096 {
                self.seen_buys.clear();
            }
            self.cur_slot = slot;
        }
    }

    /// Builds and fires the buy for `info` with a priced `plan`. Shared by the whitelist path
    /// and the confirmation trigger, so both size, patch, and fan out identically.
    #[inline]
    fn fire_launch(
        &mut self,
        info: &PumpCreateInfo,
        slot: u64,
        plan: BuyPlan,
        competition: u32,
    ) -> Option<FiredLaunch> {
        let m = &self.shared.metrics;

        // one token at a time, or stopped after a test round trip
        if !self.shared.gate.may_fire() {
            return None;
        }

        // every provider variant of this launch shares one nonce, so only one can land
        let Some((nonce_account, nonce_value)) = self.shared.nonces.take() else {
            m.skipped_no_nonce.fetch_add(1, Ordering::Relaxed);
            return None;
        };

        // The token account comes from a seed rather than the ATA program: ~18k fewer compute
        // units on chain. Both the address and the seed were derived at startup -- nothing
        // about them depends on the launch -- so this is two loads.
        self.seed_index += 1;
        if self.seed_index == self.shared.seed_table.len() {
            self.seed_index = 0;
        }
        let (seed, token_account) = self
            .shared
            .seed_table
            .get(self.seed_index, &info.token_program);

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
            // another detect thread won the race for the slot, so this launch never
            // happened. Forget the mint, or a later shred carrying the same create would be
            // dropped as a duplicate of a buy that was never made.
            self.seen.remove(&info.mint);
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
                // instruction-independent: tokens expected at the priced depth. Under
                // buy_exact_sol_in `plan.amount` is LAMPORTS, so pricing the ghost exit
                // with it would sell a nonsense token amount.
                amount: plan.expected_tokens,
                // instruction-independent: the priced budget in lamports. Under buy this
                // is the pre-slippage-pad spend (what the old max_sol_cost/(1+slip)
                // arithmetic reconstructed); under buy_exact_sol_in it is sol_in exactly.
                cost: plan.budget_lamports,
                ghost: true,
            });
            m.fired.fetch_add(1, Ordering::Relaxed);
            return Some(FiredLaunch {
                mint: info.mint,
                token_account,
                seed,
                amount: plan.amount,
                max_sol_cost: plan.max_sol_cost,
                cost_lamports: plan.budget_lamports,
                expected_tokens: plan.expected_tokens,
                bonding_curve: info.bonding_curve,
                associated_bonding_curve: info.associated_bonding_curve,
                creator: info.creator,
                token_program: info.token_program,
            });
        }

        // one copy of the body for the whole fan-out. The three fields that differ per
        // provider are stamped on by the sender threads, so every provider is queued within
        // the same microsecond instead of the last one trailing the first by ~10us.
        let base = Self::fill_body(
            &mut self.tx_ring,
            &mut self.ring_cursor,
            &self.template.tx[..len],
        );

        // per-launch tip when dynamic tip is on, else each provider's static tip. Sized off
        // the live Jito tip floor, our position size, and create-block competition (tip.rs).
        let dyn_tip = self
            .shared
            .tip
            .as_ref()
            // sized off the BUDGET, not max_sol_cost: under buy_exact_sol_in the latter is
            // a token amount and would blow the size term through the tip cap every launch.
            .map(|tp| tip::dynamic_tip(tp, tip::current(), plan.budget_lamports, competition));

        let mut queued = 0u64;
        for p in self.shared.providers.iter() {
            p.try_send(Job {
                base: base.clone(),
                tip_account: p.tip_accounts[self.launch_counter % p.tip_accounts.len()],
                // never tip a provider below its own configured minimum - a low Jito-floor
                // number can under-tip non-Jito providers (nextblock/0slot/node1) into rejection.
                tip_lamports: dyn_tip.map(|t| t.max(p.tip_lamports)).unwrap_or(p.tip_lamports),
                cu_price: p.cu_price,
            });
            queued += 1;
        }
        drop(base);

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
            // instruction-independent fields — see the ghost branch above for why the raw
            // wire args (plan.amount / plan.max_sol_cost) must not be used here.
            amount: plan.expected_tokens,
            cost: plan.budget_lamports,
            ghost: false,
        });

        Some(FiredLaunch {
            mint: info.mint,
            token_account,
            seed,
            amount: plan.amount,
            max_sol_cost: plan.max_sol_cost,
            cost_lamports: plan.budget_lamports,
            expected_tokens: plan.expected_tokens,
            bonding_curve: info.bonding_curve,
            associated_bonding_curve: info.associated_bonding_curve,
            creator: info.creator,
            token_program: info.token_program,
        })
    }
}

/// Warns when the binary was built for a baseline CPU on a box that is not one.
///
/// The crypto on this path picks its implementation at *compile* time from `target_feature`,
/// so a binary built without `-C target-cpu=native` runs the portable code on a machine that
/// has SHA-NI and AVX2 and gives no runtime hint that it is doing so. Measured here:
///
/// | stage                            | baseline | `target-cpu=native` |
/// | -------------------------------- | -------- | ------------------- |
/// | ed25519 sign                     | 11.3µs   | 9.9µs               |
/// | `create_with_seed` (one sha256)  | 150ns    | 87ns                |
///
/// `find_program_address` does not move, with or without the curve25519 SIMD backend: its
/// cost is a point decompression per candidate bump, which is a field exponentiation and is
/// not what those backends accelerate.
///
/// `scripts/bootstrap.sh` sets the flag. This exists for every other way a binary can end up
/// on the box.
#[cfg(target_arch = "x86_64")]
fn warn_if_built_without_simd() {
    if std::arch::is_x86_feature_detected!("avx2") && !cfg!(target_feature = "avx2") {
        warn!(
            "sniper: this CPU has AVX2 but the binary was not built for it. Rebuild with              RUSTFLAGS='-C target-cpu=native': signing and the token account derivation both              sit on the detect path and both get measurably faster."
        );
    }
}

#[cfg(not(target_arch = "x86_64"))]
fn warn_if_built_without_simd() {}

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
    use crate::chain::GlobalConfig;

    fn test_curve() -> CurveParams {
        CurveParams {
            initial_virtual_sol_reserves: 30_000_000_000,
            initial_virtual_token_reserves: 1_073_000_000_000_000,
            initial_real_token_reserves: 793_100_000_000_000,
            fee_basis_points: 95,
            creator_fee_basis_points: 5,
        }
    }

    /// A ghost-mode HotSniper with the whitelist disabled, so `on_create` fires without a
    /// network, a provider, or confirm mode.
    fn ghost_sniper(buy_exact_sol_in: bool, buy_lamports: u64) -> HotSniper {
        let buyer = Pubkey::new_from_array([7u8; 32]);
        let global = GlobalConfig {
            fee_recipient: Pubkey::new_from_array([1u8; 32]),
            buyback_fee_recipients: vec![Pubkey::new_from_array([2u8; 32])],
            curve: test_curve(),
            fee_basis_points: 95,
            creator_fee_basis_points: 5,
        };
        let nonces = Arc::new(chain::NoncePool::new(vec![Pubkey::new_from_array([3u8; 32])]));
        nonces.load_from_values(vec![[4u8; 32]]).unwrap();
        let static_accounts = template::StaticAccounts {
            fee_recipient: global.fee_recipient,
            buyback_fee_recipient: global.buyback_fee_recipients[0],
            user: buyer,
            user_volume_accumulator: pumpfun::user_volume_accumulator(&buyer),
        };
        let template = template::build(&static_accounts, 96_000, buy_exact_sol_in);
        // exit=true: the tracker thread returns immediately; the gate itself still works
        let exit = Arc::new(AtomicBool::new(true));
        let (gate, _handle) = position::start(
            "http://127.0.0.1:1".to_string(),
            ModeConfig { ghost_mode: true, ..ModeConfig::default() },
            test_curve(),
            exit,
        );
        let shared = Arc::new(Shared {
            whitelist: Whitelist::new(true),
            nonces,
            fee_recipients: FeeRecipientCache::new(&global),
            curve: test_curve(),
            metrics: Arc::new(SniperMetrics::default()),
            providers: Vec::new(),
            buy_lamports,
            max_dev_buy_lamports: 0,
            haircut_bps: 30,
            slippage_bps: 500,
            buyer,
            gate,
            ghost_mode: true,
            buy_exact_sol_in,
            seed_table: SeedTable::build(&buyer, 0, 4).unwrap(),
            confirm: None,
            tip: None,
        });
        let sniper = Sniper { shared, template, threads: Vec::new() };
        sniper.into_hot().0
    }

    fn launch(n: u8) -> PumpCreateInfo {
        PumpCreateInfo {
            mint: Pubkey::new_from_array([n; 32]),
            bonding_curve: Pubkey::new_from_array([n + 1; 32]),
            associated_bonding_curve: Pubkey::new_from_array([n + 2; 32]),
            creator: Pubkey::new_from_array([n + 3; 32]),
            user: Pubkey::new_from_array([n + 4; 32]),
            token_program: pumpfun::TOKEN_2022_PROGRAM,
            dev_buy_lamports: 500_000_000,
            is_v2: true,
        }
    }

    /// Regression for the exact_sol_in P0: a fired launch must carry an
    /// instruction-independent cost basis in lamports and an expected token fill,
    /// whichever buy instruction shaped the wire args. Everything downstream (ghost PnL,
    /// the Node seller's stop-loss, tip sizing) accounts against these two fields.
    #[test]
    fn fired_launch_cost_basis_is_lamports_under_both_buy_instructions() {
        let budget = 2_000_000_000u64;

        let mut exact = ghost_sniper(true, budget);
        let f = exact.on_create(&launch(10), 1).expect("ghost fire");
        assert_eq!(f.cost_lamports, budget, "exact mode: cost basis is sol_in");
        assert_eq!(f.amount, budget, "exact mode: wire arg0 is sol_in");
        assert!(
            f.max_sol_cost > 100 * budget,
            "exact mode: wire arg1 is a token floor, not lamports"
        );
        assert!(f.expected_tokens > f.max_sol_cost, "expected fill sits above the floor");

        let mut classic = ghost_sniper(false, budget);
        let f = classic.on_create(&launch(20), 1).expect("ghost fire");
        assert_eq!(f.cost_lamports, budget, "classic mode: cost basis is the pre-pad budget");
        assert_eq!(
            f.max_sol_cost,
            budget * 10_500 / 10_000,
            "classic mode: wire arg1 is the slippage-padded cap"
        );
        assert_eq!(f.expected_tokens, f.amount, "classic mode: fill is the exact request");
    }

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
