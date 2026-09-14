//! v1.1 confirmation-trigger selection, in the prod crate.
//!
//! Frozen 2026-08-25; backtest +346/+503 SOL/day trimmed on held-out days; live-paper
//! verified. Replaces the creator whitelist for the MAIN book: instead of pre-committing to a
//! deployer, watch the create block fill over the shred stream and fire on confirmed demand
//! from a *fresh* deployer, guarding against bundle sweeps and sizing with the flow.
//!
//! Decision per launch, in the order events decode:
//!
//!   create  -> dev in dev_history, or already seen this session? DEAD (not fresh).
//!   buy #k  -> buyer on the avoid list (elite operator)?   the edge is consumed; remember it.
//!              buyer on the follow list (symbiont/whale)?   FIRE the follow book, fixed size.
//!              n_conf >= N and confirmed >= MIN and no elite ahead and vq < CAP?  FIRE main.
//!
//! Everything is integer work behind one or two hash lookups; tables are `ArcSwap`'d wholesale
//! by a background reloader. Sizing is delegated to `CurveParams::plan_buy`, which the
//! whitelist path already uses, so the two books price a buy identically.

use std::collections::HashMap;
use std::hash::{BuildHasherDefault, Hasher};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use arc_swap::ArcSwapOption;
use solana_sdk::pubkey::Pubkey;

/// Set once at startup when confirm mode is on. The deshred fast path reads it to decide
/// whether to also search the payload for buy discriminators (an extra cost paid only when
/// the confirmation trigger needs create-block buys).
pub static CONFIRM_MODE: AtomicBool = AtomicBool::new(false);

#[inline]
pub fn confirm_mode() -> bool {
    CONFIRM_MODE.load(Ordering::Relaxed)
}


/// Pubkeys are uniformly distributed already; take eight bytes and use them directly.
#[derive(Default)]
pub struct IdentityHasher(u64);
impl Hasher for IdentityHasher {
    #[inline(always)]
    fn write(&mut self, bytes: &[u8]) {
        let mut buf = [0u8; 8];
        let n = bytes.len().min(8);
        buf[..n].copy_from_slice(&bytes[..n]);
        self.0 = u64::from_le_bytes(buf);
    }
    #[inline(always)]
    fn finish(&self) -> u64 {
        self.0
    }
}
type FastMap<K, V> = HashMap<K, V, BuildHasherDefault<IdentityHasher>>;

const LAMPORTS_PER_SOL: f64 = 1_000_000_000.0;
const VIRT_SOL_0: u64 = 30_000_000_000;
/// Curve invariant: initial virtual SOL (lamports) × initial virtual tokens (raw, 6 dp).
/// Matches `CurveParams` defaults; used to convert a classic buy's exact token amount into
/// its EXECUTED net spend at the tracked curve state.
const CURVE_K: u128 = 30_000_000_000u128 * 1_073_000_000_000_000u128;

fn env_u64(key: &str, default: u64) -> u64 {
    std::env::var(key).ok().and_then(|v| v.trim().parse().ok()).unwrap_or(default)
}
fn env_f64(key: &str, default: f64) -> f64 {
    std::env::var(key).ok().and_then(|v| v.trim().parse().ok()).unwrap_or(default)
}
fn sol_env(key: &str, default_sol: f64) -> u64 {
    (env_f64(key, default_sol) * LAMPORTS_PER_SOL).round() as u64
}

#[derive(Clone, Debug)]
pub struct Params {
    pub n_conf: u32,
    pub min_conf_lamports: u64,
    pub ratio_bps: u64,
    pub size_lo_lamports: u64,
    pub size_hi_lamports: u64,
    pub follow_lamports: u64,
    /// Reject when the virtual quote reserve is already at/above this: the curve is
    /// completing (a bundle swept it) or accounting is corrupt. Includes the 30 SOL offset.
    pub vq_cap_lamports: u64,
    /// A confirming buy's `sol_lamports` is its `max_sol_cost` CAP, which bots pad 10-30% over
    /// their real spend. The backtest counted actual tape spend, so the live trigger runs
    /// looser. This discounts the cap toward estimated spend (bps). 0 = off (count the raw
    /// cap); set ~1500 to tighten toward the backtest AFTER a ghost soak shows the real pad.
    /// (SNIPER_CONF_PAD_BPS)
    pub conf_pad_bps: u64,
}

impl Params {
    /// Backtested frozen defaults; every value overridable from the environment.
    pub fn from_env() -> Self {
        Params {
            n_conf: env_u64("SNIPER_N_CONF", 3) as u32,
            min_conf_lamports: sol_env("SNIPER_MIN_CONF_SOL", 4.0),
            ratio_bps: env_u64("SNIPER_SIZE_RATIO_BPS", 4700),
            size_lo_lamports: sol_env("SNIPER_SIZE_LO_SOL", 1.5),
            size_hi_lamports: sol_env("SNIPER_SIZE_HI_SOL", 3.5),
            follow_lamports: sol_env("SNIPER_FOLLOW_SIZE_SOL", 2.0),
            vq_cap_lamports: sol_env("SNIPER_VQ_CAP_SOL", 115.0),
            conf_pad_bps: env_u64("SNIPER_CONF_PAD_BPS", 0),
        }
    }

    /// Whether confirmation-trigger mode is on at all. When off, the whitelist path runs
    /// unchanged. (SNIPER_CONFIRM_MODE=1)
    pub fn enabled() -> bool {
        env_u64("SNIPER_CONFIRM_MODE", 0) == 1
    }
}

// ---------------------------------------------------------------------------
// Tables: historical dev set and watch wallets, hot-swapped by a reloader.
// ---------------------------------------------------------------------------

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WatchAction {
    FollowSymbiont,
    FollowWhale,
    Avoid,
}

#[derive(Default)]
pub struct DevSet {
    devs: FastMap<[u8; 32], ()>,
}
impl DevSet {
    pub fn len(&self) -> usize {
        self.devs.len()
    }
    pub fn is_empty(&self) -> bool {
        self.devs.is_empty()
    }
    #[inline]
    pub fn contains(&self, dev: &Pubkey) -> bool {
        self.devs.contains_key(&dev.to_bytes())
    }
    pub fn parse(text: &str) -> (Self, usize) {
        let mut set = DevSet::default();
        let mut skipped = 0usize;
        for line in text.lines() {
            let line = line.split('#').next().unwrap_or("").trim();
            if line.is_empty() {
                continue;
            }
            match line.parse::<Pubkey>() {
                Ok(k) => {
                    set.devs.insert(k.to_bytes(), ());
                }
                Err(_) => skipped += 1,
            }
        }
        (set, skipped)
    }
}

#[derive(Default)]
pub struct Watch {
    wallets: FastMap<[u8; 32], WatchAction>,
}
impl Watch {
    pub fn len(&self) -> usize {
        self.wallets.len()
    }
    #[inline]
    pub fn get(&self, wallet: &Pubkey) -> Option<WatchAction> {
        self.wallets.get(&wallet.to_bytes()).copied()
    }
    pub fn parse(text: &str) -> (Self, usize) {
        let mut w = Watch::default();
        let mut skipped = 0usize;
        for line in text.lines() {
            let line = line.split('#').next().unwrap_or("").trim();
            if line.is_empty() {
                continue;
            }
            let mut it = line.split_whitespace();
            let (key, act) = match (it.next(), it.next()) {
                (Some(a), Some(b)) => (a, b),
                _ => {
                    skipped += 1;
                    continue;
                }
            };
            let action = match act {
                "follow_symbiont" => WatchAction::FollowSymbiont,
                "follow_whale" => WatchAction::FollowWhale,
                "avoid" => WatchAction::Avoid,
                _ => {
                    skipped += 1;
                    continue;
                }
            };
            match key.parse::<Pubkey>() {
                Ok(k) => {
                    w.wallets.insert(k.to_bytes(), action);
                }
                Err(_) => skipped += 1,
            }
        }
        (w, skipped)
    }
}

/// Known high-rug creators: dev -> rug_coefficient (only >=0.3 are listed). A creator NOT in
/// this map is unknown or low-rug and passes. dev-dump rate by bucket on live tape:
/// 0.2-0.5 = 12%, 0.5-0.8 = 32%, 0.8-1 = 68%.
#[derive(Default)]
pub struct RugMap {
    rug: FastMap<[u8; 32], u16>, // rug*1000, so 0.5 -> 500
}
impl RugMap {
    pub fn len(&self) -> usize {
        self.rug.len()
    }
    /// rug_coefficient for `dev`, or 0.0 when unknown (unknown creators pass the gate).
    #[inline]
    pub fn rug(&self, dev: &Pubkey) -> f64 {
        self.rug.get(&dev.to_bytes()).map(|&r| r as f64 / 1000.0).unwrap_or(0.0)
    }
    pub fn parse(text: &str) -> (Self, usize) {
        let mut m = RugMap::default();
        let mut skipped = 0usize;
        for line in text.lines() {
            let line = line.split('#').next().unwrap_or("").trim();
            if line.is_empty() {
                continue;
            }
            let mut it = line.split_whitespace();
            match (it.next().map(str::parse::<Pubkey>), it.next().map(str::parse::<f64>)) {
                (Some(Ok(k)), Some(Ok(v))) => {
                    m.rug.insert(k.to_bytes(), (v.clamp(0.0, 1.0) * 1000.0) as u16);
                }
                _ => skipped += 1,
            }
        }
        (m, skipped)
    }
}

static DEVS: ArcSwapOption<DevSet> = ArcSwapOption::const_empty();
static WATCH: ArcSwapOption<Watch> = ArcSwapOption::const_empty();
static RUG: ArcSwapOption<RugMap> = ArcSwapOption::const_empty();

/// rug_coefficient for a creator (0.0 when unknown or no table loaded).
#[inline]
pub fn creator_rug(dev: &Pubkey) -> f64 {
    RUG.load_full().as_ref().map(|m| m.rug(dev)).unwrap_or(0.0)
}
pub fn publish_rug(m: RugMap) {
    RUG.store(Some(Arc::new(m)));
}
pub fn load_rug_from_file(path: &str) -> std::io::Result<(usize, usize)> {
    let text = std::fs::read_to_string(path)?;
    let (m, skipped) = RugMap::parse(&text);
    let n = m.len();
    publish_rug(m);
    Ok((n, skipped))
}

pub fn publish_devs(d: DevSet) {
    DEVS.store(Some(Arc::new(d)));
}
pub fn publish_watch(w: Watch) {
    WATCH.store(Some(Arc::new(w)));
}
pub fn load_devs_from_file(path: &str) -> std::io::Result<(usize, usize)> {
    let text = std::fs::read_to_string(path)?;
    let (set, skipped) = DevSet::parse(&text);
    let n = set.len();
    publish_devs(set);
    Ok((n, skipped))
}
pub fn load_watch_from_file(path: &str) -> std::io::Result<(usize, usize)> {
    let text = std::fs::read_to_string(path)?;
    let (w, skipped) = Watch::parse(&text);
    let n = w.len();
    publish_watch(w);
    Ok((n, skipped))
}

/// Reloads both tables on a cycle; a failed read keeps the previous table in place. Same
/// contract as the whitelist refresher.
pub fn spawn_reloader(
    dev_path: String,
    watch_path: String,
    rug_path: String,
    every: std::time::Duration,
) {
    std::thread::spawn(move || loop {
        let _ = load_devs_from_file(&dev_path);
        let _ = load_watch_from_file(&watch_path);
        let _ = load_rug_from_file(&rug_path);
        std::thread::sleep(every);
    });
}

// ---------------------------------------------------------------------------
// Per-launch state and the detect-thread session.
// ---------------------------------------------------------------------------

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Book {
    Main,
    FollowSymbiont,
    FollowWhale,
}

/// A fire decision. `budget_lamports` is the position size; the caller prices it through the
/// same `CurveParams::plan_buy` the whitelist path uses, so downstream is unchanged and there
/// is one pricing site. `dev_buy_lamports` is carried so the caller can price against the
/// curve state after the dev's own buy.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ConfirmFire {
    pub book: Book,
    pub budget_lamports: u64,
    /// total net curve SOL ahead of our buy (dev buy + all confirming buys). Pass this to
    /// `CurveParams::plan_buy` as the "SOL already in the curve" so the exact-token request
    /// fits under `max_sol_cost` — our buy lands on top of all of it.
    pub prior_flow_lamports: u64,
    /// confirming buys seen in the create block at fire time (for dynamic tip sizing)
    pub n_conf: u32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Pass {
    Watching,
    EliteAhead,
    CurveCapped,
    Dead,
}

/// Watches one launch's create block. `vq_lamports` accumulates the net curve SOL ahead of us
/// (dev buy + every confirming buy); the caller prices against `prior_flow = vq - VIRT_SOL_0`.
pub struct LaunchWatch {
    vq_lamports: u64,
    n_conf: u32,
    conf_lamports: u64,
    elite_ahead: bool,
    done: bool,
}

impl LaunchWatch {
    pub fn new(dev_buy_lamports: u64) -> Self {
        // vq is tracked in NET curve SOL (what actually reaches the curve). The dev buy is a
        // gross max_sol_cost, so back the 1.25% fee out to match the confirming-buy accounting.
        let dev_net = (dev_buy_lamports as u128 * 10_000 / 10_125) as u64;
        LaunchWatch {
            vq_lamports: VIRT_SOL_0.saturating_add(dev_net),
            n_conf: 0,
            conf_lamports: 0,
            elite_ahead: false,
            done: false,
        }
    }

    #[inline]
    pub fn vq_lamports(&self) -> u64 {
        self.vq_lamports
    }
    #[inline]
    pub fn is_done(&self) -> bool {
        self.done
    }

    fn fire(&mut self, book: Book, budget: u64, p: &Params) -> Result<ConfirmFire, Pass> {
        self.done = true;
        if self.vq_lamports >= p.vq_cap_lamports {
            return Err(Pass::CurveCapped);
        }
        Ok(ConfirmFire {
            book,
            budget_lamports: budget,
            // total NET curve SOL ahead of our buy (dev + every confirming buy). This is what
            // plan_buy must price against - our buy lands on TOP of all of it. Passing only the
            // dev buy underprices the curve and the request blows past max_sol_cost on chain.
            prior_flow_lamports: self.vq_lamports.saturating_sub(VIRT_SOL_0),
            n_conf: self.n_conf,
        })
    }

    /// Feed one confirming buy from the create block, in landing order. `sol_lamports` is what
    /// the buy pledged (gross of fee); `token_amount`/`exact_sol` let the EXECUTED spend be
    /// computed. Returns a fire, or why not.
    ///
    /// Classic-buy caps are padded 1.1–1.7× over real spend (measured live 2026-08-27), so
    /// counting them made `min_conf` fire at ~60% of its nominal SOL. For classic buys the
    /// instruction's exact token amount + the tracked curve state give the executed net
    /// inflow directly; `buy_exact_sol_in` pledges are already exact.
    #[inline]
    pub fn on_buy(
        &mut self,
        buyer: &Pubkey,
        sol_lamports: u64,
        token_amount: u64,
        exact_sol: bool,
        p: &Params,
    ) -> Result<ConfirmFire, Pass> {
        if self.done {
            return Err(Pass::Dead);
        }
        let action = WATCH.load_full().as_ref().and_then(|w| w.get(buyer));
        if action == Some(WatchAction::Avoid) {
            self.elite_ahead = true;
        }
        // discount the padded cap toward estimated real spend (conf_pad_bps), then take the
        // 1.25% pool fee off to get net curve inflow. NOTE two fee conventions coexist by
        // design: here we hardcode 10_125 (125 bps, matching the replay tape the strategy was
        // calibrated on); plan_buy backs out the live on-chain Global fee (fee + creator, ~100
        // bps). Same lamports, two nets - one matches the tape, one matches the chain.
        let spend = (sol_lamports as u128 * (10_000 - p.conf_pad_bps.min(10_000)) as u128
            / 10_000) as u64;
        let pledged_net = (spend as u128 * 10_000 / 10_125) as u64;
        let net = if exact_sol || token_amount == 0 {
            pledged_net
        } else {
            // executed net inflow for an exact-token buy at the current tracked curve:
            // need = K/(vTok - T) - vSol. Never above the pledged cap's net (the buy would
            // have reverted); a token amount at/over the curve falls back to the cap.
            let vsol = self.vq_lamports.max(1) as u128;
            let vtok = CURVE_K / vsol;
            match vtok.checked_sub(token_amount as u128) {
                Some(rem) if rem > 0 => {
                    let need = (CURVE_K / rem).saturating_sub(vsol) as u64;
                    need.min(pledged_net)
                }
                _ => pledged_net,
            }
        };
        self.n_conf += 1;
        self.conf_lamports = self.conf_lamports.saturating_add(net);
        self.vq_lamports = self.vq_lamports.saturating_add(net);

        match action {
            Some(WatchAction::FollowSymbiont) => {
                return self.fire(Book::FollowSymbiont, p.follow_lamports, p)
            }
            Some(WatchAction::FollowWhale) => {
                return self.fire(Book::FollowWhale, p.follow_lamports, p)
            }
            _ => {}
        }

        if self.n_conf >= p.n_conf && self.conf_lamports >= p.min_conf_lamports {
            if self.elite_ahead {
                self.done = true;
                return Err(Pass::EliteAhead);
            }
            let raw = (self.conf_lamports as u128 * p.ratio_bps as u128 / 10_000) as u64;
            let budget = raw.clamp(p.size_lo_lamports, p.size_hi_lamports);
            let budget = (budget / 100_000_000 * 100_000_000).max(p.size_lo_lamports); // snap 0.1
            return self.fire(Book::Main, budget, p);
        }
        Err(Pass::Watching)
    }
}

/// Session-local, owned by the single detect thread: every dev the proxy itself sees is
/// marked, so freshness holds between file reloads. A dev in the historical set is never fresh.
///
/// The strategy is "the dev's first launch of the UTC day", so the live-seen set is reset on
/// the UTC-day rollover. Without that a long-running proxy burns devs permanently, its fire
/// rate decays monotonically over uptime, and the map grows without bound.
pub struct Session {
    seen_devs: FastMap<[u8; 32], ()>,
    day: u64,
}
impl Default for Session {
    fn default() -> Self {
        Session { seen_devs: FastMap::default(), day: utc_day() }
    }
}

/// Days since the unix epoch, in UTC.
fn utc_day() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() / 86_400)
        .unwrap_or(0)
}

impl Session {
    #[inline]
    pub fn dev_is_fresh(&mut self, dev: &Pubkey) -> bool {
        let today = utc_day();
        if today != self.day {
            self.seen_devs.clear();
            self.day = today;
        }
        if self.seen_devs.insert(dev.to_bytes(), ()).is_some() {
            return false; // seen live this UTC day
        }
        match DEVS.load_full() {
            Some(set) => !set.contains(dev),
            None => false, // no table: refuse to call anything fresh
        }
    }
    pub fn len(&self) -> usize {
        self.seen_devs.len()
    }
    pub fn clear(&mut self) {
        self.seen_devs.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SOL: u64 = 1_000_000_000;

    // WATCH and DEVS are process-global (ArcSwap), so tests that publish into them must not
    // run concurrently or they clobber each other. Serialize on one lock.
    static TEST_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());
    fn lock() -> std::sync::MutexGuard<'static, ()> {
        TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner())
    }

    fn params() -> Params {
        Params {
            n_conf: 3,
            min_conf_lamports: 4 * SOL,
            ratio_bps: 4700,
            size_lo_lamports: 3 * SOL / 2,
            size_hi_lamports: 7 * SOL / 2,
            follow_lamports: 2 * SOL,
            vq_cap_lamports: 115 * SOL,
            conf_pad_bps: 0,
        }
    }

    fn key(n: u8) -> Pubkey {
        Pubkey::new_from_array([n; 32])
    }

    #[test]
    fn fires_on_third_buy_when_flow_is_there() {
        let _g = lock();
        publish_watch(Watch::default());
        let p = params();
        let mut lw = LaunchWatch::new(2 * SOL);
        assert_eq!(lw.on_buy(&key(1), 2 * SOL, 0, true, &p), Err(Pass::Watching));
        assert_eq!(lw.on_buy(&key(2), 2 * SOL, 0, true, &p), Err(Pass::Watching));
        let fire = lw.on_buy(&key(3), 2 * SOL, 0, true, &p).unwrap();
        assert_eq!(fire.book, Book::Main);
        // ~5.9 SOL confirmed net * 0.47 = 2.78 -> snapped to 2.7
        assert_eq!(fire.budget_lamports, 2 * SOL + 7 * SOL / 10);
        // prior flow = dev + 3 confirmers, all net of the 1.25% fee (~4 x 1.975 SOL)
        let net = 2u64 * SOL * 10_000 / 10_125;
        assert_eq!(fire.prior_flow_lamports, 4 * net);
        assert_eq!(lw.on_buy(&key(4), SOL, 0, true, &p), Err(Pass::Dead));
    }

    #[test]
    fn needs_min_sol_not_just_count() {
        let _g = lock();
        publish_watch(Watch::default());
        let p = params();
        let mut lw = LaunchWatch::new(SOL);
        for i in 1..=6u8 {
            assert_eq!(lw.on_buy(&key(i), SOL / 2, 0, true, &p), Err(Pass::Watching));
        }
        assert!(lw.on_buy(&key(7), 2 * SOL, 0, true, &p).is_ok());
    }

    #[test]
    fn prior_flow_is_net_sum_of_dev_and_all_confirmers() {
        let _g = lock();
        publish_watch(Watch::default());
        let p = params();
        let net = |gross: u64| (gross as u128 * 10_000 / 10_125) as u64;
        let mut lw = LaunchWatch::new(SOL); // dev 1 SOL gross
        lw.on_buy(&key(1), 2 * SOL, 0, true, &p).unwrap_err();
        lw.on_buy(&key(2), 3 * SOL, 0, true, &p).unwrap_err();
        let fire = lw.on_buy(&key(3), 1 * SOL, 0, true, &p).unwrap(); // 6 SOL gross confirmed -> fires
        // prior flow = net(dev) + net(2) + net(3) + net(1), all fee-adjusted
        let expected = net(SOL) + net(2 * SOL) + net(3 * SOL) + net(SOL);
        assert_eq!(fire.prior_flow_lamports, expected);
        // budget scales with confirmed flow only (excludes the dev buy): clamp then snap 0.1
        let conf = net(2 * SOL) + net(3 * SOL) + net(SOL);
        let clamped =
            ((conf as u128 * 4700 / 10_000) as u64).clamp(p.size_lo_lamports, p.size_hi_lamports);
        let want = (clamped / 100_000_000 * 100_000_000).max(p.size_lo_lamports);
        assert_eq!(fire.budget_lamports, want);
    }

    /// Classic-buy caps are padded 1.1–1.7× over real spend (measured live 2026-08-27:
    /// cap 2.125 vs spent 1.270). Counting caps let MIN_CONF_SOL=4 fire at ~2.5 SOL of real
    /// flow — the trigger must count the EXECUTED spend derived from the token amount.
    #[test]
    fn classic_buy_counts_executed_spend_not_the_padded_cap() {
        let _g = lock();
        publish_watch(Watch::default());
        let p = params();
        let mut lw = LaunchWatch::new(0);
        // a buy of T tokens at the fresh curve really costs K/(vTok-T) - vSol. Pick T so the
        // executed net is ~1 SOL, then pledge a 2x-padded cap of 2 SOL.
        let vsol = 30_000_000_000u128;
        let vtok = CURVE_K / vsol;
        let executed = 1_000_000_000u128; // 1 SOL net
        let t = (vtok - CURVE_K / (vsol + executed)) as u64;
        let padded_cap = 2_000_000_000u64; // what the wire shows
        lw.on_buy(&key(1), padded_cap, t, false, &p).unwrap_err();
        // counted flow must be ~1 SOL (executed), not ~1.975 (the cap net of fee)
        let counted = lw.vq_lamports() - VIRT_SOL_0;
        assert!(
            (900_000_000..=1_100_000_000).contains(&counted),
            "counted {counted} lamports; the padded cap would be ~1_975_000_000"
        );
        // and an exact_sol_in pledge still counts as-is (net of fee)
        let mut lw2 = LaunchWatch::new(0);
        lw2.on_buy(&key(2), 2_000_000_000, 0, true, &p).unwrap_err();
        let counted2 = lw2.vq_lamports() - VIRT_SOL_0;
        assert!((1_970_000_000..=1_980_000_000).contains(&counted2), "counted2 {counted2}");
    }

    #[test]
    fn bundle_sweep_is_capped() {
        let _g = lock();
        publish_watch(Watch::default());
        let p = params();
        let mut lw = LaunchWatch::new(0);
        lw.on_buy(&key(1), 30 * SOL, 0, true, &p).unwrap_err();
        lw.on_buy(&key(2), 30 * SOL, 0, true, &p).unwrap_err();
        assert_eq!(lw.on_buy(&key(3), 30 * SOL, 0, true, &p), Err(Pass::CurveCapped));
    }

    #[test]
    fn elite_ahead_kills_the_fire() {
        let _g = lock();
        let elite = key(200);
        let (w, skipped) = Watch::parse(&format!("{elite} avoid\n"));
        assert_eq!(skipped, 0);
        publish_watch(w);
        let p = params();
        let mut lw = LaunchWatch::new(SOL);
        assert_eq!(lw.on_buy(&elite, 2 * SOL, 0, true, &p), Err(Pass::Watching));
        assert_eq!(lw.on_buy(&key(2), 2 * SOL, 0, true, &p), Err(Pass::Watching));
        assert_eq!(lw.on_buy(&key(3), 2 * SOL, 0, true, &p), Err(Pass::EliteAhead));
    }

    #[test]
    fn follow_wallet_fires_immediately() {
        let _g = lock();
        let sym = key(201);
        let (w, _) = Watch::parse(&format!("{sym} follow_symbiont\n"));
        publish_watch(w);
        let p = params();
        let mut lw = LaunchWatch::new(SOL);
        let fire = lw.on_buy(&sym, SOL / 2, 0, true, &p).unwrap();
        assert_eq!(fire.book, Book::FollowSymbiont);
        assert_eq!(fire.budget_lamports, 2 * SOL);
    }

    #[test]
    fn dev_freshness_once_per_session_and_respects_history() {
        let _g = lock();
        let known = key(9);
        let mut set = DevSet::default();
        set.devs.insert(known.to_bytes(), ());
        publish_devs(set);
        let mut s = Session::default();
        assert!(!s.dev_is_fresh(&known));
        let new = key(10);
        assert!(s.dev_is_fresh(&new));
        assert!(!s.dev_is_fresh(&new));
    }

    #[test]
    fn rug_map_parses_and_gates_by_coefficient() {
        let a = key(1);
        let b = key(2);
        let (m, skipped) =
            RugMap::parse(&format!("# c\n{a}\t0.72\n{b}\t0.31\nbad-line\n{a}\tnotafloat\n"));
        assert_eq!(skipped, 2);
        assert!((m.rug(&a) - 0.72).abs() < 0.001);
        assert!((m.rug(&b) - 0.31).abs() < 0.001);
        assert_eq!(m.rug(&key(9)), 0.0, "unknown creator = 0.0, passes the gate");
    }

    #[test]
    fn tables_parse_and_report_skips() {
        let _g = lock();
        let good = key(1);
        let (d, skipped) = DevSet::parse(&format!("# c\n{good}\nnot-base58-0OIl\n"));
        assert_eq!((d.len(), skipped), (1, 1));
        let (w, skipped) = Watch::parse(&format!("{good} avoid\n{good} bogus\nshort\n"));
        assert_eq!((w.len(), skipped), (1, 2));
    }
}
