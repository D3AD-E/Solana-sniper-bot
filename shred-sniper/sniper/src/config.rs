//! Sniper configuration. Loaded once at startup; nothing here is touched on the hot path.

use std::{fs, path::Path};

use serde::Deserialize;
use solana_sdk::pubkey::Pubkey;

fn default_true() -> bool {
    true
}

/// How a provider wants the signed transaction wrapped.
#[derive(Debug, Clone, Copy, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "snake_case")]
pub enum BodyFormat {
    /// `sendTransaction` JSON-RPC with a base64 transaction.
    /// jito, 0slot, astralane, node1, helius sender, nozomi, blockrazor, circular.
    #[default]
    JsonRpc,
    /// `{"transaction":{"content":"<base64>"},...}` — nextblock and bloxroute style.
    Wrapped,
    /// `Wrapped` plus bloXroute's `submitProtection`. Their default is `SP_MEDIUM`, which
    /// *holds* a transaction until four consecutive slots are clear of a leader they score
    /// as high-risk. That is a sane default for a swap and fatal for a create-block snipe,
    /// where the edge is gone by slip 2 — so bloxroute gets its own format pinned to
    /// `SP_LOW` rather than sharing nextblock's. `"low"` is rejected: the enum spelling is
    /// the one `/api/v2/submit` parses.
    WrappedBlox,
    /// `{"transaction":"<base64>"}` — lucum and blockrazor.
    PlainTx,
    /// `{"transactions":["<base64>"]}` — flashblock's submit-batch.
    Batch,
    /// Raw transaction bytes as the request body, `application/octet-stream`, auth in the
    /// query string — blockrazor's `/v2/sendBinaryTransaction`. Skips base64 entirely, so
    /// the request is ~26% smaller than the JSON form and the hot path does no encoding.
    Binary,
}

#[derive(Debug, Clone, Deserialize)]
pub struct ProviderConfig {
    /// free-form label used in logs and metrics
    pub name: String,
    /// every regional endpoint of this provider. A host may carry its own path suffix
    /// (`newyork.solana.blockrazor.xyz/sendTransaction`) for providers that publish one.
    #[serde(default)]
    pub hosts: Vec<String>,
    /// single hostname, still accepted
    #[serde(default)]
    pub host: String,
    #[serde(default = "ProviderConfig::default_port")]
    pub port: u16,
    /// request path, e.g. `/?api-key=...`
    #[serde(default = "ProviderConfig::default_path")]
    pub path: String,
    #[serde(default)]
    pub tls: bool,
    #[serde(default)]
    pub body: BodyFormat,
    /// extra request headers, e.g. `[["api-key", "..."]]`
    #[serde(default)]
    pub headers: Vec<(String, String)>,
    /// tip accounts this provider accepts; rotated per launch. A single `tip_account` is
    /// still accepted for backwards compatibility.
    #[serde(default)]
    pub tip_accounts: Vec<String>,
    #[serde(default)]
    pub tip_account: String,
    /// tip in lamports; must clear the provider's documented minimum
    pub tip_lamports: u64,
    /// compute unit price in micro-lamports
    #[serde(default)]
    pub cu_price: u64,
    /// keep-alive probe path (GET, carrying this provider's headers). Empty disables probing.
    #[serde(default)]
    pub health_path: String,
    #[serde(default = "default_true")]
    pub enabled: bool,
}

impl ProviderConfig {
    fn default_port() -> u16 {
        80
    }
    fn default_path() -> String {
        "/".to_string()
    }

    /// (host, path) for every endpoint, folding in the legacy single `host`.
    pub fn endpoints(&self) -> Vec<(String, String)> {
        let mut all = self.hosts.clone();
        if !self.host.is_empty() {
            all.push(self.host.clone());
        }
        all.into_iter()
            .map(|h| match h.find('/') {
                Some(i) => (h[..i].to_string(), h[i..].to_string()),
                None => (h, self.path.clone()),
            })
            .collect()
    }

    /// All tip accounts as raw bytes, with the legacy `tip_account` folded in.
    pub fn tip_account_bytes(&self) -> Result<Vec<[u8; 32]>, String> {
        let mut all = self.tip_accounts.clone();
        if !self.tip_account.is_empty() {
            all.push(self.tip_account.clone());
        }
        if all.is_empty() {
            return Err(format!("provider {} has no tip accounts", self.name));
        }
        all.iter()
            .map(|s| {
                s.parse::<Pubkey>()
                    .map(|p| p.to_bytes())
                    .map_err(|e| format!("provider {}: bad tip account {s}: {e}", self.name))
            })
            .collect()
    }
}

#[derive(Debug, Clone, Deserialize)]
pub struct SniperConfig {
    /// path to a solana keypair json (the 64-byte secret array)
    pub keypair_path: String,
    /// newline separated base58 pubkeys allowed to launch; reloaded in the background
    #[serde(default)]
    pub whitelist_path: String,
    /// when true, every launch passes the whitelist check (use for shadow testing)
    #[serde(default)]
    pub whitelist_disabled: bool,
    /// local validator / RPC used for the nonce accounts and the pump global config.
    /// Never on the hot path.
    pub rpc_url: String,
    /// durable nonce accounts, base58. One is used per launch; every provider variant of
    /// that launch shares it, so only one of them can ever land.
    #[serde(default)]
    pub nonce_accounts: Vec<String>,
    /// total lamports to spend per launch, fees included
    #[serde(default = "SniperConfig::default_buy_lamports")]
    pub buy_lamports: u64,
    /// skip the launch when the dev buy is at least this large, in lamports
    #[serde(default)]
    pub max_dev_buy_lamports: u64,
    /// shaved off the requested token amount so a curve that moved slightly further than
    /// expected still fits under the cap
    #[serde(default = "SniperConfig::default_haircut_bps")]
    pub haircut_bps: u64,
    /// headroom on `max_sol_cost` only; never spent unless the curve actually moved
    #[serde(default = "SniperConfig::default_slippage_bps")]
    pub slippage_bps: u64,
    /// compute unit limit. The reference transaction uses 90k and burns 68k.
    #[serde(default = "SniperConfig::default_cu_limit")]
    pub cu_limit: u32,
    /// nonce refresh interval
    #[serde(default = "SniperConfig::default_nonce_ms")]
    pub nonce_refresh_ms: u64,
    /// How long a sender thread keeps spinning on its queue after the last job before it
    /// parks. A parked thread costs the *detect* thread a ~6.4us kernel wake per provider;
    /// a spinning one costs ~150ns but burns a core while it spins. 0 disables spinning.
    #[serde(default = "SniperConfig::default_sender_spin_micros")]
    pub sender_spin_micros: u64,
    /// One token at a time: nothing new is bought until the open position closes or its
    /// buy is known to have failed. Off means positions may overlap.
    #[serde(default = "SniperConfig::default_true")]
    pub sync_mode: bool,
    /// Stop after one completed round trip, a buy that landed and a sell that closed it.
    #[serde(default)]
    pub test_mode: bool,
    /// Never touch the chain. Buys are priced, recorded and exited on paper against the real
    /// curve, so a strategy can be measured without spending anything.
    #[serde(default)]
    pub ghost_mode: bool,
    /// How long a ghost position is held before it is priced out.
    #[serde(default = "SniperConfig::default_hold_ms")]
    pub hold_ms: u64,
    /// How often the chain is polled while a position is open.
    #[serde(default = "SniperConfig::default_poll_ms")]
    pub position_poll_ms: u64,
    /// After this long with no token account, a buy is treated as lost and the gate lifts.
    #[serde(default = "SniperConfig::default_buy_timeout_ms")]
    pub buy_timeout_ms: u64,
    /// dry run: build, patch and sign, but never write to a socket
    #[serde(default)]
    pub dry_run: bool,
    pub providers: Vec<ProviderConfig>,
}

impl SniperConfig {
    fn default_buy_lamports() -> u64 {
        1_000_000_000
    }
    fn default_haircut_bps() -> u64 {
        30
    }
    fn default_slippage_bps() -> u64 {
        100
    }
    fn default_cu_limit() -> u32 {
        90_000
    }
    fn default_nonce_ms() -> u64 {
        300
    }
    fn default_sender_spin_micros() -> u64 {
        2_000_000
    }
    fn default_true() -> bool {
        true
    }
    fn default_hold_ms() -> u64 {
        1_600
    }
    fn default_poll_ms() -> u64 {
        200
    }
    fn default_buy_timeout_ms() -> u64 {
        30_000
    }

    pub fn load(path: &Path) -> Result<Self, String> {
        let raw = fs::read_to_string(path).map_err(|e| format!("read {path:?}: {e}"))?;
        let mut cfg: SniperConfig =
            serde_json::from_str(&raw).map_err(|e| format!("parse {path:?}: {e}"))?;
        cfg.apply_env_overrides();
        Ok(cfg)
    }

    /// The scalar strategy knobs are ALSO read from the environment, and the environment WINS.
    /// sniper.json carries provider endpoints + API keys (generated once); these numbers change
    /// with the strategy, so making `.env` authoritative gives one source of truth and stops a
    /// stale json (e.g. clobbered by an rsync deploy) from silently running old slippage/CU.
    /// The env names match gen-config's, so a regenerated json and a live override agree.
    fn apply_env_overrides(&mut self) {
        fn num(key: &str) -> Option<u64> {
            std::env::var(key).ok().and_then(|v| v.trim().parse().ok())
        }
        fn flag(key: &str) -> Option<bool> {
            std::env::var(key).ok().map(|v| v.trim() == "1")
        }
        if let Some(v) = num("SNIPER_BUY_LAMPORTS") {
            self.buy_lamports = v;
        }
        if let Some(v) = num("SNIPER_MAX_DEV_BUY_LAMPORTS") {
            self.max_dev_buy_lamports = v;
        }
        if let Some(v) = num("SNIPER_HAIRCUT_BPS") {
            self.haircut_bps = v;
        }
        if let Some(v) = num("SNIPER_SLIPPAGE_BPS") {
            self.slippage_bps = v;
        }
        if let Some(v) = num("SNIPER_CU_LIMIT") {
            self.cu_limit = v as u32;
        }
        if let Some(v) = num("SNIPER_HOLD_MS") {
            self.hold_ms = v;
        }
        if let Some(v) = num("SNIPER_POSITION_POLL_MS") {
            self.position_poll_ms = v;
        }
        if let Some(v) = num("SNIPER_BUY_TIMEOUT_MS") {
            self.buy_timeout_ms = v;
        }
        if let Some(v) = num("SNIPER_NONCE_REFRESH_MS") {
            self.nonce_refresh_ms = v;
        }
        // SNIPER_SYNC_MODE defaults to on unless explicitly "0"
        if let Ok(v) = std::env::var("SNIPER_SYNC_MODE") {
            self.sync_mode = v.trim() != "0";
        }
        if let Some(v) = flag("SNIPER_TEST_MODE") {
            self.test_mode = v;
        }
        if let Some(v) = flag("SNIPER_GHOST_MODE") {
            self.ghost_mode = v;
        }
        if let Some(v) = flag("SNIPER_DRY_RUN") {
            self.dry_run = v;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tip_accounts_merge_the_legacy_single_field() {
        let cfg: ProviderConfig = serde_json::from_str(
            r#"{"name":"t","host":"h","tip_account":"11111111111111111111111111111111",
                "tip_accounts":["TokenkegQfeZyiNwAJbNbGKPFXCWuBvf9Ss623VQ5DA"],
                "tip_lamports":1}"#,
        )
        .unwrap();
        assert_eq!(cfg.tip_account_bytes().unwrap().len(), 2);
        assert_eq!(cfg.body, BodyFormat::JsonRpc);
    }

    #[test]
    fn missing_tip_accounts_are_rejected() {
        let cfg: ProviderConfig =
            serde_json::from_str(r#"{"name":"t","host":"h","tip_lamports":1}"#).unwrap();
        assert!(cfg.tip_account_bytes().is_err());
    }

    #[test]
    fn defaults_buy_one_sol() {
        let cfg: SniperConfig =
            serde_json::from_str(r#"{"keypair_path":"k","rpc_url":"r","providers":[]}"#).unwrap();
        assert_eq!(cfg.buy_lamports, 1_000_000_000);
        assert_eq!(cfg.cu_limit, 90_000);
        assert_eq!(cfg.slippage_bps, 100);
        assert_eq!(cfg.haircut_bps, 30);
    }

    #[test]
    fn sync_mode_is_on_and_the_other_modes_are_off_by_default() {
        let cfg: SniperConfig =
            serde_json::from_str(r#"{"keypair_path":"k","rpc_url":"r","providers":[]}"#).unwrap();
        assert!(cfg.sync_mode, "one token at a time unless asked otherwise");
        assert!(!cfg.test_mode);
        assert!(!cfg.ghost_mode);
    }
}
