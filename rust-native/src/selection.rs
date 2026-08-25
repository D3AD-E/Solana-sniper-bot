//! Launch selection for the buy path.
//!
//! This runs between "a pump.fun `create` was reconstructed from shreds" and "sign and fire",
//! so it sits directly on the latency budget. Everything here is therefore allocation-free,
//! branch-light integer work: one hash lookup plus a handful of u128 multiplies.
//!
//! What it decides, and why (measured against 24678QKx…, 19,879 buys over 15 days):
//!
//!   * creator whitelist - all of that wallet's buys came from 195 deployer addresses out of
//!     the ~31,700 launches a day on pump. Nothing else is worth a transaction.
//!   * per-creator dev-buy floor - it only fires when the deployer put in at least its own
//!     standard amount. That single rule accounts for 88% of the launches it passes on.
//!   * depth band - entries into a curve holding under 5 SOL lose money; 5-18 SOL is where
//!     the wave shows up. The upper bound is enforceable for free through `max_sol_cost`,
//!     which is what produces the 6002 reverts when a launch is over-subscribed.
//!   * per-creator budget and CU price - the budget is the real position size; the token
//!     amount handed to the pump `buy` instruction is derived from it.
//!   * rolling per-creator result - a creator whose recent snipes lost money is switched off
//!     until it recovers. This is the single most valuable filter found: on his own launches
//!     it roughly triples the edge per attempt and is the only variant that stayed profitable
//!     on a degraded day.
//!
//! The registry is swapped wholesale by a background reloader, so the hot path never takes a
//! lock and never sees a half-written table.

use arc_swap::ArcSwapOption;
use std::collections::HashMap;
use std::fs;
use std::hash::{BuildHasherDefault, Hasher};
use std::sync::Arc;

/// pump.fun bonding curve, in raw units: 30 SOL and 1.073e9 tokens of virtual reserves.
pub const VIRT_SOL_0: u64 = 30_000_000_000;
pub const VIRT_TOKENS_0: u64 = 1_073_000_000_000_000;
/// Pool fee charged on top of the curve cost, in basis points (1.25% as observed on-chain).
pub const FEE_BPS: u64 = 125;

/// Pubkeys are already uniformly distributed, so hashing them again is wasted work on the hot
/// path. Take eight bytes and use them directly.
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

#[derive(Clone, Copy, Debug)]
pub struct CreatorCfg {
    /// Minimum lamports the deployer must put into its own launch.
    pub floor_lamports: u64,
    /// Position size: this becomes `max_sol_cost` on the buy instruction.
    pub budget_lamports: u64,
    /// Compute-unit price in micro-lamports, against a fixed 110k CU limit.
    pub cu_price: u64,
    /// Reject when the curve already holds less than this (no confirming flow yet).
    pub min_depth_lamports: u64,
    /// Reject when the curve already holds more than this (we are too late).
    pub max_depth_lamports: u64,
}

/// Rolling result for one creator, updated as our own trades close.
///
/// A plain ring of the last N outcomes rather than an average: the point is to switch a
/// creator off quickly when it turns, and a long-run mean is too slow to do that.
#[derive(Clone, Copy, Debug)]
pub struct CreatorState {
    ring: [f32; Self::WINDOW],
    len: u8,
    next: u8,
}

impl Default for CreatorState {
    fn default() -> Self {
        CreatorState { ring: [0.0; Self::WINDOW], len: 0, next: 0 }
    }
}

impl CreatorState {
    pub const WINDOW: usize = 32;
    /// Trade this creator until we have seen at least this many results.
    pub const WARMUP: u8 = 10;

    pub fn record(&mut self, pnl_sol: f32) {
        self.ring[self.next as usize] = pnl_sol;
        self.next = (self.next + 1) % Self::WINDOW as u8;
        if (self.len as usize) < Self::WINDOW {
            self.len += 1;
        }
    }

    #[inline]
    pub fn mean(&self) -> f32 {
        if self.len == 0 {
            return 0.0;
        }
        let n = self.len as usize;
        self.ring[..n].iter().sum::<f32>() / n as f32
    }

    /// Warming up counts as enabled: we cannot rank a creator we have never traded.
    #[inline]
    pub fn enabled(&self) -> bool {
        self.len < Self::WARMUP || self.mean() > 0.0
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Plan {
    /// `amount` argument for the pump `buy` instruction, in raw token units.
    pub token_amount: u64,
    /// `max_sol_cost` argument, in lamports.
    pub max_sol_cost: u64,
    pub cu_price: u64,
}

/// Why a launch was passed on. Kept as a discriminant so the caller can count rejections
/// without formatting anything on the hot path.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Reject {
    NotWhitelisted,
    DevBuyBelowFloor,
    TooShallow,
    TooDeep,
    Unfillable,
    CreatorCold,
}

pub struct Registry {
    creators: FastMap<[u8; 32], CreatorCfg>,
}

/// Our own trading results, kept apart from the config so a config reload never wipes them.
#[derive(Default)]
pub struct Results {
    per_creator: FastMap<[u8; 32], CreatorState>,
}

impl Results {
    #[inline]
    pub fn get(&self, creator: &[u8; 32]) -> CreatorState {
        self.per_creator.get(creator).copied().unwrap_or_default()
    }

    /// Call when a position closes, with the realised PnL of that round trip.
    pub fn record(&mut self, creator: [u8; 32], pnl_sol: f32) {
        self.per_creator.entry(creator).or_default().record(pnl_sol);
    }

    pub fn cold(&self) -> usize {
        self.per_creator.values().filter(|s| !s.enabled()).count()
    }
}

impl Registry {
    pub fn empty() -> Self {
        Registry { creators: FastMap::default() }
    }

    pub fn len(&self) -> usize {
        self.creators.len()
    }

    pub fn is_empty(&self) -> bool {
        self.creators.is_empty()
    }

    /// The whole decision. `curve_quote_lamports` is the curve's quote reserve *including* the
    /// 30 SOL virtual offset, i.e. what the create (plus anything already ahead of us in the
    /// block) left behind. `state` carries our own recent results for this creator.
    #[inline]
    pub fn decide(
        &self,
        creator: &[u8; 32],
        dev_buy_lamports: u64,
        curve_quote_lamports: u64,
        state: &CreatorState,
    ) -> Result<Plan, Reject> {
        let cfg = self.creators.get(creator).ok_or(Reject::NotWhitelisted)?;

        if !state.enabled() {
            return Err(Reject::CreatorCold);
        }

        if dev_buy_lamports < cfg.floor_lamports {
            return Err(Reject::DevBuyBelowFloor);
        }

        let depth = curve_quote_lamports.saturating_sub(VIRT_SOL_0);
        if depth < cfg.min_depth_lamports {
            return Err(Reject::TooShallow);
        }
        if depth > cfg.max_depth_lamports {
            return Err(Reject::TooDeep);
        }

        let token_amount = tokens_for_budget(curve_quote_lamports, cfg.budget_lamports)
            .ok_or(Reject::Unfillable)?;
        if token_amount == 0 {
            return Err(Reject::Unfillable);
        }

        Ok(Plan {
            token_amount,
            max_sol_cost: cfg.budget_lamports,
            cu_price: cfg.cu_price,
        })
    }

    pub fn get(&self, creator: &[u8; 32]) -> Option<&CreatorCfg> {
        self.creators.get(creator)
    }

    pub fn insert(&mut self, creator: [u8; 32], cfg: CreatorCfg) {
        self.creators.insert(creator, cfg);
    }
}

/// Token reserve implied by the quote reserve. The curve is a constant product on the virtual
/// reserves, so `k` never changes and the token side follows from the quote side alone.
#[inline(always)]
fn tokens_at(quote_lamports: u64) -> u128 {
    const K: u128 = VIRT_SOL_0 as u128 * VIRT_TOKENS_0 as u128;
    K / quote_lamports as u128
}

/// How many raw tokens a budget buys, given the curve state we expect to land in.
///
/// The budget is `max_sol_cost`, which has to cover both the curve cost and the pool fee
/// charged on top of it, so the amount that actually reaches the curve is the budget less
/// that fee. Asking for the tokens that exactly consume the budget would revert on any
/// adverse movement, so this is deliberately the fillable amount rather than the maximum one.
#[inline]
pub fn tokens_for_budget(quote_lamports: u64, budget_lamports: u64) -> Option<u64> {
    if quote_lamports == 0 || budget_lamports == 0 {
        return None;
    }
    let into_curve = (budget_lamports as u128 * 10_000) / (10_000 + FEE_BPS as u128);
    if into_curve == 0 {
        return None;
    }
    let before = tokens_at(quote_lamports);
    let after = {
        const K: u128 = VIRT_SOL_0 as u128 * VIRT_TOKENS_0 as u128;
        K / (quote_lamports as u128 + into_curve)
    };
    u64::try_from(before.saturating_sub(after)).ok()
}

/// Lamports the curve will charge for `token_amount`, fee excluded. Useful for sizing checks
/// and for the backtest to agree with the on-chain program.
#[inline]
pub fn cost_for_tokens(quote_lamports: u64, token_amount: u64) -> Option<u64> {
    const K: u128 = VIRT_SOL_0 as u128 * VIRT_TOKENS_0 as u128;
    let before = tokens_at(quote_lamports);
    let after = before.checked_sub(token_amount as u128)?;
    if after == 0 {
        return None;
    }
    u64::try_from((K / after).saturating_sub(quote_lamports as u128)).ok()
}

// ---------------------------------------------------------------------------
// Config loading. Tab separated, one creator per line:
//
//   <creator base58>  <floor SOL>  <budget SOL>  <cu price>  <min depth SOL>  <max depth SOL>
//
// Blank lines and anything after a '#' are ignored, so the file stays hand-editable while a
// generator writes it.
// ---------------------------------------------------------------------------

fn sol_to_lamports(s: &str) -> Option<u64> {
    let v: f64 = s.trim().parse().ok()?;
    if !v.is_finite() || v < 0.0 {
        return None;
    }
    Some((v * 1_000_000_000.0).round() as u64)
}

pub fn parse_registry(text: &str) -> (Registry, usize) {
    let mut reg = Registry::empty();
    let mut skipped = 0usize;
    for line in text.lines() {
        let line = line.split('#').next().unwrap_or("").trim();
        if line.is_empty() {
            continue;
        }
        let mut it = line.split_whitespace();
        let (key, floor, budget, cu) = match (it.next(), it.next(), it.next(), it.next()) {
            (Some(a), Some(b), Some(c), Some(d)) => (a, b, c, d),
            _ => {
                skipped += 1;
                continue;
            }
        };
        let min_depth = it.next().unwrap_or("5");
        let max_depth = it.next().unwrap_or("18");

        let decoded = match bs58_decode_32(key) {
            Some(k) => k,
            None => {
                skipped += 1;
                continue;
            }
        };
        let cfg = match (
            sol_to_lamports(floor),
            sol_to_lamports(budget),
            cu.trim().parse::<u64>().ok(),
            sol_to_lamports(min_depth),
            sol_to_lamports(max_depth),
        ) {
            (Some(f), Some(b), Some(c), Some(mn), Some(mx)) => CreatorCfg {
                floor_lamports: f,
                budget_lamports: b,
                cu_price: c,
                min_depth_lamports: mn,
                max_depth_lamports: mx,
            },
            _ => {
                skipped += 1;
                continue;
            }
        };
        reg.insert(decoded, cfg);
    }
    (reg, skipped)
}

/// Minimal base58 decode for 32-byte keys. Avoids pulling a dependency into the buy path.
pub fn bs58_decode_32(s: &str) -> Option<[u8; 32]> {
    const ALPHABET: &[u8; 58] = b"123456789ABCDEFGHJKLMNPQRSTUVWXYZabcdefghijkmnopqrstuvwxyz";
    let mut map = [255u8; 128];
    for (i, c) in ALPHABET.iter().enumerate() {
        map[*c as usize] = i as u8;
    }
    let mut out = [0u8; 32];
    for c in s.bytes() {
        if c >= 128 {
            return None;
        }
        let val = map[c as usize];
        if val == 255 {
            return None;
        }
        let mut carry = val as u32;
        for byte in out.iter_mut().rev() {
            let cur = (*byte as u32) * 58 + carry;
            *byte = (cur & 0xff) as u8;
            carry = cur >> 8;
        }
        if carry != 0 {
            return None; // overflowed 32 bytes
        }
    }
    Some(out)
}

// ---------------------------------------------------------------------------
// Hot-swappable global registry.
//
// The buy path only ever loads an Arc; the reloader publishes a whole new table. ArcSwap makes
// the load wait-free and, unlike swapping a raw pointer by hand, guarantees a reader that has
// already grabbed the old table keeps it alive until it is done with it.
// ---------------------------------------------------------------------------

static CURRENT: ArcSwapOption<Registry> = ArcSwapOption::const_empty();

pub fn publish(reg: Registry) {
    CURRENT.store(Some(Arc::new(reg)));
}

#[inline]
pub fn current() -> Option<Arc<Registry>> {
    CURRENT.load_full()
}

/// Loads the file and publishes it. Returns (creators, skipped lines).
pub fn load_from_file(path: &str) -> std::io::Result<(usize, usize)> {
    let text = fs::read_to_string(path)?;
    let (reg, skipped) = parse_registry(&text);
    let n = reg.len();
    publish(reg);
    Ok((n, skipped))
}

/// Background reloader, so the whitelist can rotate without a restart. The buy path is never
/// blocked by it and a failed read simply leaves the previous table in place.
pub fn spawn_reloader(path: String, every: std::time::Duration) {
    std::thread::spawn(move || loop {
        let _ = load_from_file(&path);
        std::thread::sleep(every);
    });
}

/// Convenience for the shred handler: look up the live table and decide in one call.
#[inline]
pub fn decide_now(
    creator: &[u8; 32],
    dev_buy_lamports: u64,
    curve_quote_lamports: u64,
    state: &CreatorState,
) -> Result<Plan, Reject> {
    match current() {
        Some(reg) => reg.decide(creator, dev_buy_lamports, curve_quote_lamports, state),
        None => Err(Reject::NotWhitelisted),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const DEV: &str = "whamNNP9tHoxLg92yHvJPdYhghEoCg1qYTsh5a2oLbx";

    fn reg() -> Registry {
        let (r, skipped) = parse_registry(&format!(
            "# creator\tfloor\tbudget\tcu\tmin_depth\tmax_depth\n{DEV}\t5.00\t0.50\t10000000\t5\t18\n"
        ));
        assert_eq!(skipped, 0);
        r
    }

    #[test]
    fn base58_roundtrip_len() {
        assert!(bs58_decode_32(DEV).is_some());
        assert!(bs58_decode_32("not-base58-0OIl").is_none());
    }

    #[test]
    fn curve_matches_observed_create() {
        // From replay: a 1.843591186 SOL dev buy left 1_010_878_446.842777 tokens in the curve.
        let quote = VIRT_SOL_0 + 1_843_591_186;
        let remaining = tokens_at(quote) / 1_000_000; // raw -> whole tokens
        assert!((remaining as i64 - 1_010_878_446).abs() <= 1, "got {remaining}");
    }

    #[test]
    fn budget_and_cost_agree() {
        let quote = VIRT_SOL_0 + 8_000_000_000;
        let budget = 1_500_000_000u64;
        let tokens = tokens_for_budget(quote, budget).unwrap();
        let cost = cost_for_tokens(quote, tokens).unwrap();
        let with_fee = cost + cost * FEE_BPS / 10_000;
        assert!(with_fee <= budget, "cost {with_fee} over budget {budget}");
        assert!(with_fee * 100 / budget >= 98, "budget badly underused: {with_fee}");
    }

    #[test]
    fn rejects_in_priority_order() {
        let r = reg();
        let dev = bs58_decode_32(DEV).unwrap();
        let other = [7u8; 32];
        let depth_ok = VIRT_SOL_0 + 8_000_000_000;

        let warm = CreatorState::default();
        assert_eq!(r.decide(&other, 5_000_000_000, depth_ok, &warm), Err(Reject::NotWhitelisted));
        assert_eq!(r.decide(&dev, 4_000_000_000, depth_ok, &warm), Err(Reject::DevBuyBelowFloor));
        assert_eq!(
            r.decide(&dev, 5_000_000_000, VIRT_SOL_0 + 1_000_000_000, &warm),
            Err(Reject::TooShallow)
        );
        assert_eq!(
            r.decide(&dev, 5_000_000_000, VIRT_SOL_0 + 40_000_000_000, &warm),
            Err(Reject::TooDeep)
        );

        let plan = r.decide(&dev, 5_000_000_000, depth_ok, &warm).unwrap();
        assert_eq!(plan.max_sol_cost, 500_000_000);
        assert_eq!(plan.cu_price, 10_000_000);
        assert!(plan.token_amount > 0);
    }

    #[test]
    fn published_table_is_visible_to_the_hot_path() {
        publish(reg());
        let dev = bs58_decode_32(DEV).unwrap();
        let quote = VIRT_SOL_0 + 8_000_000_000;
        let warm = CreatorState::default();
        assert!(decide_now(&dev, 5_000_000_000, quote, &warm).is_ok());
        publish(Registry::empty());
        assert_eq!(decide_now(&dev, 5_000_000_000, quote, &warm), Err(Reject::NotWhitelisted));
    }

    #[test]
    fn a_losing_creator_is_switched_off_and_can_recover() {
        let r = reg();
        let dev = bs58_decode_32(DEV).unwrap();
        let quote = VIRT_SOL_0 + 8_000_000_000;
        let mut results = Results::default();

        // warmup: traded even with nothing but losses recorded so far
        for _ in 0..(CreatorState::WARMUP - 1) {
            results.record(dev, -0.05);
        }
        assert!(r.decide(&dev, 5_000_000_000, quote, &results.get(&dev)).is_ok());

        // once warmed up on losses it goes cold
        results.record(dev, -0.05);
        assert_eq!(
            r.decide(&dev, 5_000_000_000, quote, &results.get(&dev)),
            Err(Reject::CreatorCold)
        );

        // and a run of wins brings it back
        for _ in 0..20 {
            results.record(dev, 0.5);
        }
        assert!(r.decide(&dev, 5_000_000_000, quote, &results.get(&dev)).is_ok());
    }

    #[test]
    fn deeper_curve_buys_fewer_tokens() {
        let shallow = tokens_for_budget(VIRT_SOL_0 + 5_000_000_000, 1_000_000_000).unwrap();
        let deep = tokens_for_budget(VIRT_SOL_0 + 15_000_000_000, 1_000_000_000).unwrap();
        assert!(deep < shallow, "price should rise with depth: {deep} vs {shallow}");
    }
}
