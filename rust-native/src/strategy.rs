//! v1.1 confirmation-trigger strategy for the buy path.
//!
//! This replaces creator-whitelist selection (selection.rs) with the strategy backtested in
//! analysis/ (frozen 2026-08-25, +346/+503 SOL/day trimmed on the held-out days, live
//! paper-verified): watch the create block fill over shreds, fire on confirmed demand from a
//! fresh deployer, guard against bundle sweeps, size with the flow.
//!
//! Decision per launch, in the order the events decode:
//!
//!   create  -> dev in the historical dev set, or already seen this session? DEAD (not fresh).
//!   buy #k  -> buyer on the avoid list (elite operator)?   remember; the edge is consumed.
//!              buyer on the follow list (symbiont/whale)?  FIRE follow-book, fixed size.
//!              n_conf >= N and confirmed >= MIN_CONF and no elite ahead
//!                and vq < VQ_CAP (bundle-sweep / absurd-state guard)?  FIRE main book,
//!                size = clamp(RATIO x confirmed, LO, HI).
//!
//! Everything on the hot path is one or two hash lookups plus integer math - no allocation,
//! no locks. Tables (dev history, watch wallets) are ArcSwap'd wholesale by background
//! reloaders fed from Postgres via analysis/export_tables.py + analysis/dev_sweep.py.
//! All tunables come from the environment (see `.env`, `Params::from_env`).

use crate::selection::{bs58_decode_32, tokens_for_budget, IdentityHasher, VIRT_SOL_0};
use arc_swap::ArcSwapOption;
use std::collections::HashMap;
use std::fs;
use std::hash::BuildHasherDefault;
use std::sync::Arc;

type FastMap<K, V> = HashMap<K, V, BuildHasherDefault<IdentityHasher>>;

const LAMPORTS_PER_SOL: f64 = 1_000_000_000.0;

// ---------------------------------------------------------------------------
// Tunables, all from the environment so the ops surface is one .env file.
// ---------------------------------------------------------------------------

#[derive(Clone, Debug)]
pub struct Params {
    /// Fire after this many confirming buys in the create block. (SNIPER_N_CONF)
    pub n_conf: u32,
    /// ...and at least this much net SOL confirmed. (SNIPER_MIN_CONF_SOL)
    pub min_conf_lamports: u64,
    /// size = clamp(ratio x confirmed, lo, hi), snapped to 0.1 SOL. (SNIPER_SIZE_RATIO_BPS)
    pub ratio_bps: u64,
    pub size_lo_lamports: u64, // SNIPER_SIZE_LO_SOL
    pub size_hi_lamports: u64, // SNIPER_SIZE_HI_SOL
    /// Fixed size for follow-book entries behind a symbiont/whale. (SNIPER_FOLLOW_SIZE_SOL)
    pub follow_lamports: u64,
    /// Reject when the virtual quote reserve is already at/above this: the curve is
    /// completing - a bundle swept it - or our accounting is corrupt. Either way there is
    /// nothing to price. (SNIPER_VQ_CAP_SOL, virtual, i.e. includes the 30 SOL offset)
    pub vq_cap_lamports: u64,
    pub cu_limit: u32,     // SNIPER_CU_LIMIT
    pub cu_price: u64,     // SNIPER_CU_PRICE (micro-lamports)
    pub tip_lamports: u64, // SNIPER_TIP_LAMPORTS
    /// Exit ladder: (slot offset from entry, basis points of the position).
    /// (SNIPER_EXIT_LEGS="1:3000,6:2000,9:2000,13:1500,18:1500")
    pub exit_legs: Vec<(u32, u16)>,
    /// Force-out slot offset. (SNIPER_EXIT_LAST_SLOT)
    pub exit_last_slot: u32,
    /// Dump everything when value multiple <= this, in bps. (SNIPER_STOP_X_BPS)
    pub stop_x_bps: u32,
    /// Above this multiple, trim moon_frac instead of the scheduled leg. (SNIPER_MOON_X_BPS)
    pub moon_x_bps: u32,
    pub moon_frac_bps: u16, // SNIPER_MOON_FRAC_BPS
}

fn env_f64(key: &str, default: f64) -> f64 {
    std::env::var(key).ok().and_then(|v| v.trim().parse().ok()).unwrap_or(default)
}

fn env_u64(key: &str, default: u64) -> u64 {
    std::env::var(key).ok().and_then(|v| v.trim().parse().ok()).unwrap_or(default)
}

fn sol_env(key: &str, default_sol: f64) -> u64 {
    (env_f64(key, default_sol) * LAMPORTS_PER_SOL).round() as u64
}

fn parse_legs(s: &str) -> Option<Vec<(u32, u16)>> {
    let mut out = Vec::new();
    for part in s.split(',') {
        let (slot, bps) = part.trim().split_once(':')?;
        out.push((slot.trim().parse().ok()?, bps.trim().parse().ok()?));
    }
    if out.is_empty() {
        return None;
    }
    Some(out)
}

impl Params {
    /// Backtested frozen defaults; every value can be overridden from the environment.
    pub fn from_env() -> Self {
        let legs = std::env::var("SNIPER_EXIT_LEGS")
            .ok()
            .and_then(|s| parse_legs(&s))
            .unwrap_or_else(|| vec![(1, 3000), (6, 2000), (9, 2000), (13, 1500), (18, 1500)]);
        Params {
            n_conf: env_u64("SNIPER_N_CONF", 3) as u32,
            min_conf_lamports: sol_env("SNIPER_MIN_CONF_SOL", 4.0),
            ratio_bps: env_u64("SNIPER_SIZE_RATIO_BPS", 4700),
            size_lo_lamports: sol_env("SNIPER_SIZE_LO_SOL", 1.5),
            size_hi_lamports: sol_env("SNIPER_SIZE_HI_SOL", 3.5),
            follow_lamports: sol_env("SNIPER_FOLLOW_SIZE_SOL", 2.0),
            vq_cap_lamports: sol_env("SNIPER_VQ_CAP_SOL", 115.0),
            cu_limit: env_u64("SNIPER_CU_LIMIT", 90_000) as u32,
            cu_price: env_u64("SNIPER_CU_PRICE", 6_000_000),
            tip_lamports: env_u64("SNIPER_TIP_LAMPORTS", 2_000_000),
            exit_legs: legs,
            exit_last_slot: env_u64("SNIPER_EXIT_LAST_SLOT", 24) as u32,
            stop_x_bps: env_u64("SNIPER_STOP_X_BPS", 8_000) as u32,
            moon_x_bps: env_u64("SNIPER_MOON_X_BPS", 20_000) as u32,
            moon_frac_bps: env_u64("SNIPER_MOON_FRAC_BPS", 500) as u16,
        }
    }
}

// ---------------------------------------------------------------------------
// Tables: historical dev set and the watch wallets, hot-swapped like the registry.
// ---------------------------------------------------------------------------

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WatchAction {
    /// Symbiont insider: their buy is the entry signal.
    FollowSymbiont,
    /// Whale: same, separate book for accounting.
    FollowWhale,
    /// Elite operator: if one landed before our trigger, skip the launch.
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
    pub fn contains(&self, dev: &[u8; 32]) -> bool {
        self.devs.contains_key(dev)
    }
    /// One base58 pubkey per line; '#' comments allowed.
    pub fn parse(text: &str) -> (Self, usize) {
        let mut set = DevSet::default();
        let mut skipped = 0usize;
        for line in text.lines() {
            let line = line.split('#').next().unwrap_or("").trim();
            if line.is_empty() {
                continue;
            }
            match bs58_decode_32(line) {
                Some(k) => {
                    set.devs.insert(k, ());
                }
                None => skipped += 1,
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
    pub fn get(&self, wallet: &[u8; 32]) -> Option<WatchAction> {
        self.wallets.get(wallet).copied()
    }
    /// Tab/space separated: `<wallet base58> <follow_symbiont|follow_whale|avoid>`.
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
            match bs58_decode_32(key) {
                Some(k) => {
                    w.wallets.insert(k, action);
                }
                None => skipped += 1,
            }
        }
        (w, skipped)
    }
}

static DEVS: ArcSwapOption<DevSet> = ArcSwapOption::const_empty();
static WATCH: ArcSwapOption<Watch> = ArcSwapOption::const_empty();

pub fn publish_devs(d: DevSet) {
    DEVS.store(Some(Arc::new(d)));
}
pub fn publish_watch(w: Watch) {
    WATCH.store(Some(Arc::new(w)));
}

pub fn load_devs_from_file(path: &str) -> std::io::Result<(usize, usize)> {
    let text = fs::read_to_string(path)?;
    let (set, skipped) = DevSet::parse(&text);
    let n = set.len();
    publish_devs(set);
    Ok((n, skipped))
}

pub fn load_watch_from_file(path: &str) -> std::io::Result<(usize, usize)> {
    let text = fs::read_to_string(path)?;
    let (w, skipped) = Watch::parse(&text);
    let n = w.len();
    publish_watch(w);
    Ok((n, skipped))
}

/// Reloads both tables on a cycle; a failed read keeps the previous table, same contract as
/// selection::spawn_reloader.
pub fn spawn_reloader(dev_path: String, watch_path: String, every: std::time::Duration) {
    std::thread::spawn(move || loop {
        let _ = load_devs_from_file(&dev_path);
        let _ = load_watch_from_file(&watch_path);
        std::thread::sleep(every);
    });
}

// ---------------------------------------------------------------------------
// Session-local state, owned by the (single) detect thread. Devs seen live are fresh only
// once; the historical set from the file catches everything older.
// ---------------------------------------------------------------------------

#[derive(Default)]
pub struct Session {
    seen_devs: FastMap<[u8; 32], ()>,
}

impl Session {
    /// True exactly once per dev per session, and never for a dev in the historical set.
    /// Marks the dev as seen either way.
    #[inline]
    pub fn dev_is_fresh(&mut self, dev: &[u8; 32]) -> bool {
        let seen_live = self.seen_devs.insert(*dev, ()).is_some();
        if seen_live {
            return false;
        }
        match DEVS.load_full() {
            Some(set) => !set.contains(dev),
            // no table published: refuse to call anything fresh
            None => false,
        }
    }
}

// ---------------------------------------------------------------------------
// The per-launch state machine.
// ---------------------------------------------------------------------------

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Book {
    Main,
    FollowSymbiont,
    FollowWhale,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Fire {
    pub book: Book,
    /// Position size; becomes `max_sol_cost` on the pump buy instruction.
    pub max_sol_cost: u64,
    /// `amount` for the buy instruction, derived from the curve state we expect to land in.
    pub token_amount: u64,
    pub cu_price: u64,
    pub cu_limit: u32,
    pub tip_lamports: u64,
}

/// Why the launch was passed on. `Dead` means no more work will ever be done on it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Pass {
    /// Still watching; feed the next event.
    Watching,
    /// Trigger met but an elite operator landed first - edge consumed.
    EliteAhead,
    /// Curve at/above the cap: bundle sweep or corrupt accounting.
    CurveCapped,
    /// Sizing produced nothing fillable.
    Unfillable,
    /// Already fired or already declared dead.
    Dead,
}

pub struct LaunchWatch {
    /// Virtual quote reserve, offset included, tracked as create-block events decode.
    vq_lamports: u64,
    n_conf: u32,
    conf_lamports: u64,
    elite_ahead: bool,
    done: bool,
}

impl LaunchWatch {
    /// `dev_buy_net_lamports`: what the create's initial buy put INTO the curve (fee excluded).
    pub fn new(dev_buy_net_lamports: u64) -> Self {
        LaunchWatch {
            vq_lamports: VIRT_SOL_0 + dev_buy_net_lamports,
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

    /// A sell decoded in the create block (rare): curve drains, nothing confirms.
    #[inline]
    pub fn on_sell(&mut self, net_lamports: u64) {
        self.vq_lamports = self.vq_lamports.saturating_sub(net_lamports).max(VIRT_SOL_0);
    }

    fn fire(&mut self, book: Book, size: u64, p: &Params) -> Result<Fire, Pass> {
        if self.vq_lamports >= p.vq_cap_lamports {
            self.done = true;
            return Err(Pass::CurveCapped);
        }
        let token_amount = match tokens_for_budget(self.vq_lamports, size) {
            Some(t) if t > 0 => t,
            _ => {
                self.done = true;
                return Err(Pass::Unfillable);
            }
        };
        self.done = true;
        Ok(Fire {
            book,
            max_sol_cost: size,
            token_amount,
            cu_price: p.cu_price,
            cu_limit: p.cu_limit,
            tip_lamports: p.tip_lamports,
        })
    }

    /// Feed one confirming buy from the create block, in landing order.
    /// `net_lamports` is what reached the curve (quote net of the 1.25% fee).
    #[inline]
    pub fn on_buy(
        &mut self,
        buyer: &[u8; 32],
        net_lamports: u64,
        p: &Params,
    ) -> Result<Fire, Pass> {
        if self.done {
            return Err(Pass::Dead);
        }
        let action = WATCH.load_full().as_ref().and_then(|w| w.get(buyer));
        if action == Some(WatchAction::Avoid) {
            self.elite_ahead = true;
        }
        self.n_conf += 1;
        self.conf_lamports += net_lamports;
        self.vq_lamports += net_lamports;

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
            let size = raw.clamp(p.size_lo_lamports, p.size_hi_lamports);
            let size = size / 100_000_000 * 100_000_000; // snap down to 0.1 SOL
            return self.fire(Book::Main, size.max(p.size_lo_lamports), p);
        }
        Err(Pass::Watching)
    }

    /// The create block sealed without a trigger; nothing more will happen here.
    pub fn seal(&mut self) {
        self.done = true;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SOL: u64 = 1_000_000_000;
    const W1: &str = "whamNNP9tHoxLg92yHvJPdYhghEoCg1qYTsh5a2oLbx";

    fn params() -> Params {
        Params {
            n_conf: 3,
            min_conf_lamports: 4 * SOL,
            ratio_bps: 4700,
            size_lo_lamports: 3 * SOL / 2,
            size_hi_lamports: 7 * SOL / 2,
            follow_lamports: 2 * SOL,
            vq_cap_lamports: 115 * SOL,
            cu_limit: 90_000,
            cu_price: 6_000_000,
            tip_lamports: 2_000_000,
            exit_legs: vec![(1, 3000), (6, 2000), (9, 2000), (13, 1500), (18, 1500)],
            exit_last_slot: 24,
            stop_x_bps: 8000,
            moon_x_bps: 20000,
            moon_frac_bps: 500,
        }
    }

    fn key(n: u8) -> [u8; 32] {
        [n; 32]
    }

    #[test]
    fn fires_on_third_buy_when_flow_is_there() {
        publish_watch(Watch::default());
        let p = params();
        let mut lw = LaunchWatch::new(2 * SOL); // dev put 2 in
        assert_eq!(lw.on_buy(&key(1), 2 * SOL, &p), Err(Pass::Watching));
        assert_eq!(lw.on_buy(&key(2), 2 * SOL, &p), Err(Pass::Watching));
        let fire = lw.on_buy(&key(3), 2 * SOL, &p).unwrap();
        assert_eq!(fire.book, Book::Main);
        // 6 SOL confirmed * 0.47 = 2.82 -> snapped to 2.8
        assert_eq!(fire.max_sol_cost, 2 * SOL + 8 * SOL / 10);
        assert!(fire.token_amount > 0);
        // and it never fires twice
        assert_eq!(lw.on_buy(&key(4), SOL, &p), Err(Pass::Dead));
    }

    #[test]
    fn needs_min_sol_not_just_count() {
        publish_watch(Watch::default());
        let p = params();
        let mut lw = LaunchWatch::new(SOL);
        for i in 1..=5u8 {
            assert_eq!(lw.on_buy(&key(i), SOL / 2, &p), Err(Pass::Watching));
        }
        // 6th half-SOL buy crosses 3.0 total... still below 4.0
        assert_eq!(lw.on_buy(&key(6), SOL / 2, &p), Err(Pass::Watching));
        let fire = lw.on_buy(&key(7), 2 * SOL, &p).unwrap(); // 5.0 confirmed now
        assert_eq!(fire.book, Book::Main);
        // 5.0 * 0.47 = 2.35 -> 2.3
        assert_eq!(fire.max_sol_cost, 2 * SOL + 3 * SOL / 10);
    }

    #[test]
    fn sizing_clamps_both_ends() {
        publish_watch(Watch::default());
        let p = params();
        // tiny flow -> floor
        let mut lw = LaunchWatch::new(0);
        lw.on_buy(&key(1), SOL, &p).unwrap_err();
        lw.on_buy(&key(2), SOL, &p).unwrap_err();
        let f = lw.on_buy(&key(3), 2 * SOL, &p).unwrap(); // 4.0 * .47 = 1.88 -> 1.8
        assert_eq!(f.max_sol_cost, SOL + 8 * SOL / 10);
        // huge flow -> cap
        let mut lw = LaunchWatch::new(0);
        lw.on_buy(&key(1), 10 * SOL, &p).unwrap_err();
        lw.on_buy(&key(2), 10 * SOL, &p).unwrap_err();
        let f = lw.on_buy(&key(3), 10 * SOL, &p).unwrap();
        assert_eq!(f.max_sol_cost, 7 * SOL / 2);
    }

    #[test]
    fn bundle_sweep_is_capped() {
        publish_watch(Watch::default());
        let p = params();
        let mut lw = LaunchWatch::new(0);
        lw.on_buy(&key(1), 30 * SOL, &p).unwrap_err();
        lw.on_buy(&key(2), 30 * SOL, &p).unwrap_err();
        assert_eq!(lw.on_buy(&key(3), 30 * SOL, &p), Err(Pass::CurveCapped));
        assert_eq!(lw.on_buy(&key(4), SOL, &p), Err(Pass::Dead));
    }

    #[test]
    fn elite_ahead_kills_the_fire() {
        let elite = bs58_decode_32(W1).unwrap();
        let (w, skipped) = Watch::parse(&format!("{W1} avoid\n"));
        assert_eq!(skipped, 0);
        publish_watch(w);
        let p = params();
        let mut lw = LaunchWatch::new(SOL);
        assert_eq!(lw.on_buy(&elite, 2 * SOL, &p), Err(Pass::Watching));
        assert_eq!(lw.on_buy(&key(2), 2 * SOL, &p), Err(Pass::Watching));
        assert_eq!(lw.on_buy(&key(3), 2 * SOL, &p), Err(Pass::EliteAhead));
    }

    #[test]
    fn follow_wallet_fires_immediately_at_fixed_size() {
        let sym = bs58_decode_32(W1).unwrap();
        let (w, _) = Watch::parse(&format!("{W1} follow_symbiont\n"));
        publish_watch(w);
        let p = params();
        let mut lw = LaunchWatch::new(SOL);
        let fire = lw.on_buy(&sym, SOL / 2, &p).unwrap();
        assert_eq!(fire.book, Book::FollowSymbiont);
        assert_eq!(fire.max_sol_cost, 2 * SOL);
    }

    #[test]
    fn dev_freshness_is_once_per_session_and_respects_history() {
        let known = key(9);
        let mut set = DevSet::default();
        set.devs.insert(known, ());
        publish_devs(set);
        let mut s = Session::default();
        assert!(!s.dev_is_fresh(&known)); // in history
        let new = key(10);
        assert!(s.dev_is_fresh(&new)); // first time live
        assert!(!s.dev_is_fresh(&new)); // second launch same session
    }

    #[test]
    fn tables_parse_and_report_skips() {
        let (d, skipped) = DevSet::parse(&format!("# comment\n{W1}\nnot-base58-0OIl\n"));
        assert_eq!((d.len(), skipped), (1, 1));
        let (w, skipped) = Watch::parse(&format!("{W1} avoid\n{W1} bogus\nshort\n"));
        assert_eq!((w.len(), skipped), (1, 2));
    }

    #[test]
    fn legs_parse_from_env_format() {
        let legs = parse_legs("1:3000, 6:2000,9:2000").unwrap();
        assert_eq!(legs, vec![(1, 3000), (6, 2000), (9, 2000)]);
        assert!(parse_legs("").is_none());
        assert!(parse_legs("1-3000").is_none());
    }

    #[test]
    fn sells_drain_but_never_below_virtual() {
        publish_watch(Watch::default());
        let mut lw = LaunchWatch::new(SOL);
        lw.on_sell(5 * SOL);
        assert_eq!(lw.vq_lamports(), VIRT_SOL_0);
    }
}
