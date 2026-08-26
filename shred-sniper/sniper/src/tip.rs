//! Dynamic Jito tip.
//!
//! The static tip in the config gets outbid on hot launches and overpays on quiet ones. This
//! sizes the tip per launch from two grounded inputs:
//!
//!   * the live Jito tip floor. Jito publishes the landed-tip percentiles (25/50/75/95/99th)
//!     at `https://bundles.jito.wtf/api/v1/bundles/tip_floor`; a background thread polls it
//!     and `ArcSwap`s the result. Paying the 75th percentile is the common bot baseline
//!     (e.g. 1fge/pump-fun-sniper-bot), so a tip is competitive without overpaying.
//!   * position size and create-block competition. Measured on E4EzXdwf's 195 landed buys,
//!     his tip tracks his own SIZE most (spearman +0.67), then market urgency (priority
//!     +0.24, block competition +0.20); entry rank is ~0. So the tip scales with our size
//!     and bumps on crowded blocks.
//!
//! tip = clamp( max(floor_pctile, size * size_bps) * (1 + block_buys * comp_step_bps), min, max )
//!
//! All integer work on the hot path behind one `ArcSwap` load. Off unless SNIPER_DYNAMIC_TIP=1,
//! in which case the per-launch tip replaces the provider's static tip for every provider; the
//! static value is the fallback when the stream is unavailable.

use std::sync::Arc;
use std::time::Duration;

use arc_swap::ArcSwapOption;

const LAMPORTS_PER_SOL: f64 = 1_000_000_000.0;

/// Landed-tip percentiles from the Jito tip floor, in lamports.
#[derive(Clone, Copy, Debug, Default)]
pub struct TipFloor {
    pub p50: u64,
    pub p75: u64,
    pub p95: u64,
    pub p99: u64,
}

static FLOOR: ArcSwapOption<TipFloor> = ArcSwapOption::const_empty();

pub fn publish(f: TipFloor) {
    FLOOR.store(Some(Arc::new(f)));
}
#[inline]
pub fn current() -> Option<TipFloor> {
    FLOOR.load_full().map(|a| *a)
}

#[derive(Clone, Copy, Debug)]
pub struct TipParams {
    /// Which floor percentile to use as the market baseline: 50, 75, 95, or 99.
    pub pctile: u8,
    /// Tip as basis points of position size (E4Ez ~58 bps: tip median 0.0115 / size 1.98).
    pub size_bps: u64,
    /// Extra tip per competing create-block buy, in basis points (crowded block -> pay up).
    pub comp_step_bps: u64,
    pub min_lamports: u64,
    pub max_lamports: u64,
    /// Used when the tip stream has not published yet (the provider's static tip).
    pub fallback_lamports: u64,
}

fn env_u64(key: &str, default: u64) -> u64 {
    std::env::var(key).ok().and_then(|v| v.trim().parse().ok()).unwrap_or(default)
}
fn env_f64(key: &str, default: f64) -> f64 {
    std::env::var(key).ok().and_then(|v| v.trim().parse().ok()).unwrap_or(default)
}
fn sol_env(key: &str, default_sol: f64) -> u64 {
    (env_f64(key, default_sol) * LAMPORTS_PER_SOL).round() as u64
}

impl TipParams {
    /// None unless SNIPER_DYNAMIC_TIP=1. `fallback` is the provider's static tip.
    pub fn from_env(fallback_lamports: u64) -> Option<Self> {
        if env_u64("SNIPER_DYNAMIC_TIP", 0) != 1 {
            return None;
        }
        Some(TipParams {
            pctile: env_u64("SNIPER_TIP_PCTILE", 75) as u8,
            size_bps: env_u64("SNIPER_TIP_SIZE_BPS", 58),
            comp_step_bps: env_u64("SNIPER_TIP_COMP_STEP_BPS", 300),
            min_lamports: sol_env("SNIPER_TIP_MIN_SOL", 0.006),
            max_lamports: sol_env("SNIPER_TIP_MAX_SOL", 0.05),
            fallback_lamports,
        })
    }

    #[inline]
    fn floor_at(&self, f: &TipFloor) -> u64 {
        match self.pctile {
            0..=50 => f.p50,
            51..=75 => f.p75,
            76..=95 => f.p95,
            _ => f.p99,
        }
    }
}

/// The per-launch tip. `size_lamports` is the position size (max_sol_cost); `block_buys` is the
/// number of confirming buys in the create block (0 on the whitelist path).
#[inline]
pub fn dynamic_tip(p: &TipParams, floor: Option<TipFloor>, size_lamports: u64, block_buys: u32) -> u64 {
    let market = floor.map(|f| p.floor_at(&f)).unwrap_or(p.fallback_lamports);
    let size_term = (size_lamports as u128 * p.size_bps as u128 / 10_000) as u64;
    let base = market.max(size_term);
    let comp_num = 10_000u128 + block_buys as u128 * p.comp_step_bps as u128;
    let tip = (base as u128 * comp_num / 10_000) as u64;
    tip.clamp(p.min_lamports, p.max_lamports)
}

/// Polls the Jito tip floor and publishes it. A failed poll keeps the previous value; the hot
/// path falls back to the static tip until the first successful poll.
pub fn spawn_poller(url: String, every: Duration) {
    std::thread::spawn(move || {
        let client = reqwest::blocking::Client::builder()
            .timeout(Duration::from_secs(5))
            .build()
            .ok();
        loop {
            if let Some(c) = &client {
                if let Ok(resp) = c.get(&url).send().and_then(|r| r.text()) {
                    if let Some(f) = parse_tip_floor(&resp) {
                        publish(f);
                    }
                }
            }
            std::thread::sleep(every);
        }
    });
}

/// Parses the Jito tip_floor JSON: an array whose last element carries the percentile fields
/// (in SOL). Kept tolerant so a schema addition does not break it.
pub fn parse_tip_floor(body: &str) -> Option<TipFloor> {
    let v: serde_json::Value = serde_json::from_str(body).ok()?;
    let obj = match &v {
        serde_json::Value::Array(a) => a.last()?,
        _ => &v,
    };
    let sol = |k: &str| -> u64 {
        obj.get(k)
            .and_then(|x| x.as_f64())
            .map(|s| (s * LAMPORTS_PER_SOL).round() as u64)
            .unwrap_or(0)
    };
    let f = TipFloor {
        p50: sol("landed_tips_50th_percentile"),
        p75: sol("landed_tips_75th_percentile"),
        p95: sol("landed_tips_95th_percentile"),
        p99: sol("landed_tips_99th_percentile"),
    };
    if f.p50 == 0 && f.p75 == 0 {
        return None; // nothing parsed
    }
    Some(f)
}

#[cfg(test)]
mod tests {
    use super::*;

    const SOL: u64 = 1_000_000_000;

    fn params() -> TipParams {
        TipParams {
            pctile: 75,
            size_bps: 58,
            comp_step_bps: 300,
            min_lamports: SOL / 1000 * 6, // 0.006
            max_lamports: SOL / 100 * 5,  // 0.05
            fallback_lamports: 2_000_000, // 0.002 static
        }
    }

    fn floor() -> TipFloor {
        TipFloor {
            p50: 11_500_000, // 0.0115
            p75: 16_600_000, // 0.0166
            p95: 68_900_000,
            p99: 138_000_000,
        }
    }

    #[test]
    fn scales_with_size_when_market_is_quiet() {
        let p = params();
        // a bare floor of 0.0166 (p75); size 2 SOL * 58bps = 0.0116 -> market wins
        let t = dynamic_tip(&p, Some(floor()), 2 * SOL, 0);
        assert_eq!(t, 16_600_000);
        // size 3.5 SOL * 58bps = 0.0203 > p75 0.0166 -> size wins
        let t = dynamic_tip(&p, Some(floor()), 7 * SOL / 2, 0);
        assert_eq!(t, 20_300_000);
    }

    #[test]
    fn crowded_block_bumps_the_tip() {
        let p = params();
        let calm = dynamic_tip(&p, Some(floor()), 2 * SOL, 0);
        let busy = dynamic_tip(&p, Some(floor()), 2 * SOL, 8); // +24%
        assert!(busy > calm);
        assert_eq!(busy, 16_600_000 * (10_000 + 8 * 300) / 10_000);
    }

    #[test]
    fn clamps_to_min_and_max() {
        let p = params();
        // tiny size, no floor -> fallback 0.002, below min 0.006 -> min
        let t = dynamic_tip(&p, None, SOL / 100, 0);
        assert_eq!(t, p.min_lamports);
        // huge size and crowded -> would exceed max, clamp
        let t = dynamic_tip(&p, Some(floor()), 100 * SOL, 20);
        assert_eq!(t, p.max_lamports);
    }

    #[test]
    fn falls_back_when_stream_is_down() {
        let mut p = params();
        p.min_lamports = 0;
        // no floor -> uses fallback as market baseline
        let t = dynamic_tip(&p, None, 0, 0);
        assert_eq!(t, p.fallback_lamports);
    }

    #[test]
    fn parses_the_jito_tip_floor_json() {
        let body = r#"[{"time":"t","landed_tips_25th_percentile":6e-6,
            "landed_tips_50th_percentile":0.0000115,"landed_tips_75th_percentile":0.0000166,
            "landed_tips_95th_percentile":0.001,"landed_tips_99th_percentile":0.010008}]"#;
        let f = parse_tip_floor(body).unwrap();
        assert_eq!(f.p50, 11_500);
        assert_eq!(f.p75, 16_600);
        assert_eq!(f.p99, 10_008_000);
        assert!(parse_tip_floor("not json").is_none());
        assert!(parse_tip_floor("[]").is_none());
    }
}
