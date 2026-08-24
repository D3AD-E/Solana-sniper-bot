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
    /// `{"transaction":"<base64>"}` — lucum and blockrazor.
    PlainTx,
    /// `{"transactions":["<base64>"]}` — flashblock's submit-batch.
    Batch,
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
    /// keep-alive probe path (GET). Empty disables probing.
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

    pub fn load(path: &Path) -> Result<Self, String> {
        let raw = fs::read_to_string(path).map_err(|e| format!("read {path:?}: {e}"))?;
        serde_json::from_str(&raw).map_err(|e| format!("parse {path:?}: {e}"))
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
}
