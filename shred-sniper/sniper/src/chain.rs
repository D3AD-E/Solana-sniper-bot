//! Everything that talks to an RPC node. All of it runs on background threads; the hot
//! path only reads an `ArcSwap`.

use std::{
    sync::{
        atomic::{AtomicBool, AtomicUsize, Ordering},
        Arc,
    },
    thread::{Builder, JoinHandle},
    time::Duration,
};

use arc_swap::ArcSwap;
use log::{debug, warn};
use solana_client::rpc_client::RpcClient;
use solana_sdk::{commitment_config::CommitmentConfig, pubkey::Pubkey};

use crate::pumpfun::{CurveParams, GLOBAL};

/// Fields of the pump.fun `Global` account that we need.
#[derive(Debug, Clone)]
pub struct GlobalConfig {
    pub fee_recipient: Pubkey,
    pub buyback_fee_recipients: Vec<Pubkey>,
    pub curve: CurveParams,
    /// protocol fee, basis points
    pub fee_basis_points: u64,
    /// creator fee, basis points
    pub creator_fee_basis_points: u64,
}

/// Parses the borsh layout of the on-chain `Global` account (anchor discriminator first).
///
/// Offsets follow the deployed IDL: initialized(1) authority(32) fee_recipient(32)
/// initial_virtual_token_reserves(8) initial_virtual_sol_reserves(8)
/// initial_real_token_reserves(8) token_total_supply(8) fee_basis_points(8)
/// withdraw_authority(32) enable_migrate(1) pool_migration_fee(8)
/// creator_fee_basis_points(8) fee_recipients(32*7) set_creator_authority(32)
/// admin_set_creator_authority(32) create_v2_enabled(1) whitelist_pda(32)
/// reserved_fee_recipient(32) mayhem_mode_enabled(1) reserved_fee_recipients(32*7)
/// is_cashback_enabled(1) buyback_fee_recipients(32*8) ...
pub fn parse_global(data: &[u8]) -> Result<GlobalConfig, String> {
    let mut o = 8usize;
    let take = |o: &mut usize, n: usize| -> Result<&[u8], String> {
        let end = *o + n;
        let s = data
            .get(*o..end)
            .ok_or_else(|| format!("global account too short at {o}"))?;
        *o = end;
        Ok(s)
    };
    let pk = |o: &mut usize| -> Result<Pubkey, String> {
        let b: [u8; 32] = take(o, 32)?.try_into().map_err(|_| "pubkey".to_string())?;
        Ok(Pubkey::new_from_array(b))
    };
    let u64le = |o: &mut usize| -> Result<u64, String> {
        let b: [u8; 8] = take(o, 8)?.try_into().map_err(|_| "u64".to_string())?;
        Ok(u64::from_le_bytes(b))
    };

    take(&mut o, 1)?; // initialized
    let _authority = pk(&mut o)?;
    let fee_recipient = pk(&mut o)?;
    let initial_virtual_token_reserves = u64le(&mut o)?;
    let initial_virtual_sol_reserves = u64le(&mut o)?;
    let initial_real_token_reserves = u64le(&mut o)?;
    let _token_total_supply = u64le(&mut o)?;
    let fee_basis_points = u64le(&mut o)?;
    let _withdraw_authority = pk(&mut o)?;
    take(&mut o, 1)?; // enable_migrate
    let _pool_migration_fee = u64le(&mut o)?;
    let creator_fee_basis_points = u64le(&mut o)?;
    for _ in 0..7 {
        pk(&mut o)?; // fee_recipients
    }
    let _set_creator_authority = pk(&mut o)?;
    let _admin_set_creator_authority = pk(&mut o)?;
    take(&mut o, 1)?; // create_v2_enabled
    let _whitelist_pda = pk(&mut o)?;
    let _reserved_fee_recipient = pk(&mut o)?;
    take(&mut o, 1)?; // mayhem_mode_enabled
    for _ in 0..7 {
        pk(&mut o)?; // reserved_fee_recipients
    }
    take(&mut o, 1)?; // is_cashback_enabled
    let mut buyback_fee_recipients = Vec::with_capacity(8);
    for _ in 0..8 {
        buyback_fee_recipients.push(pk(&mut o)?);
    }

    Ok(GlobalConfig {
        fee_recipient,
        buyback_fee_recipients,
        curve: CurveParams {
            initial_virtual_sol_reserves,
            initial_virtual_token_reserves,
            initial_real_token_reserves,
            fee_basis_points,
            creator_fee_basis_points,
        },
        fee_basis_points,
        creator_fee_basis_points,
    })
}

pub fn fetch_global(rpc_url: &str) -> Result<GlobalConfig, String> {
    let client = RpcClient::new_with_commitment(rpc_url.to_string(), CommitmentConfig::confirmed());
    let data = client
        .get_account_data(&GLOBAL)
        .map_err(|e| format!("fetch global account: {e}"))?;
    parse_global(&data)
}

/// Fee recipients as raw bytes, refreshed in the background.
///
/// pump.fun rotates `fee_recipient` and picks a buyback recipient per transaction. Baking
/// either into the template at startup means every buy starts failing the moment the config
/// rotates, so both are patched per launch from here.
#[derive(Clone)]
pub struct FeeRecipientCache {
    fee_recipient: Arc<ArcSwap<[u8; 32]>>,
    buyback: Arc<ArcSwap<Vec<[u8; 32]>>>,
}

impl FeeRecipientCache {
    pub fn new(global: &GlobalConfig) -> Self {
        Self {
            fee_recipient: Arc::new(ArcSwap::from_pointee(global.fee_recipient.to_bytes())),
            buyback: Arc::new(ArcSwap::from_pointee(
                global
                    .buyback_fee_recipients
                    .iter()
                    .map(|p| p.to_bytes())
                    .collect::<Vec<_>>(),
            )),
        }
    }

    #[inline(always)]
    pub fn fee_recipient(&self) -> [u8; 32] {
        **self.fee_recipient.load()
    }

    /// Round-robins the buyback recipient, the way live buys on chain do.
    #[inline(always)]
    pub fn buyback(&self, nth: usize) -> [u8; 32] {
        let list = self.buyback.load();
        if list.is_empty() {
            return [0u8; 32];
        }
        list[nth % list.len()]
    }

    pub fn spawn_refresher(
        &self,
        rpc_url: String,
        interval_secs: u64,
        exit: Arc<AtomicBool>,
    ) -> JoinHandle<()> {
        let fee_recipient = self.fee_recipient.clone();
        let buyback = self.buyback.clone();
        Builder::new()
            .name("snipeGlobal".to_string())
            .spawn(move || {
                while !exit.load(Ordering::Relaxed) {
                    std::thread::sleep(Duration::from_secs(interval_secs));
                    match fetch_global(&rpc_url) {
                        Ok(g) => {
                            fee_recipient.store(Arc::new(g.fee_recipient.to_bytes()));
                            buyback.store(Arc::new(
                                g.buyback_fee_recipients
                                    .iter()
                                    .map(|p| p.to_bytes())
                                    .collect::<Vec<_>>(),
                            ));
                        }
                        Err(e) => warn!("global refresh failed: {e}"),
                    }
                }
            })
            .expect("spawn global refresher")
    }
}


/// A pool of durable nonce accounts.
///
/// One nonce account can back only one landed transaction per advance, so a single account
/// silently kills every launch after the first one inside the same window: the second
/// transaction carries a nonce the first already advanced and is rejected by every provider.
/// The pool hands out a different account per launch and refreshes their values in the
/// background.
///
/// Every provider variant of a *single* launch deliberately shares one nonce, which makes
/// the duplicate-buy problem impossible: whichever provider lands first advances the nonce
/// and the rest become invalid.
pub struct NoncePool {
    accounts: Vec<[u8; 32]>,
    values: Arc<ArcSwap<Vec<[u8; 32]>>>,
    next: AtomicUsize,
}

impl NoncePool {
    pub fn new(accounts: Vec<Pubkey>) -> Self {
        let n = accounts.len();
        Self {
            accounts: accounts.iter().map(|p| p.to_bytes()).collect(),
            values: Arc::new(ArcSwap::from_pointee(vec![[0u8; 32]; n])),
            next: AtomicUsize::new(0),
        }
    }

    pub fn len(&self) -> usize {
        self.accounts.len()
    }

    pub fn is_empty(&self) -> bool {
        self.accounts.is_empty()
    }

    /// Hot path. Returns (nonce account, current nonce value), skipping accounts whose
    /// value has not been read yet.
    #[inline(always)]
    pub fn take(&self) -> Option<([u8; 32], [u8; 32])> {
        if self.accounts.is_empty() {
            return None;
        }
        let values = self.values.load();
        let start = self.next.fetch_add(1, Ordering::Relaxed);
        for i in 0..self.accounts.len() {
            let idx = (start + i) % self.accounts.len();
            let value = values[idx];
            if value != [0u8; 32] {
                return Some((self.accounts[idx], value));
            }
        }
        None
    }

    /// Reads every nonce account and validates it before the sniper is allowed to arm.
    pub fn load_once(&self, rpc_url: &str, expected_authority: &Pubkey) -> Result<(), String> {
        let client =
            RpcClient::new_with_commitment(rpc_url.to_string(), CommitmentConfig::confirmed());
        let keys = self
            .accounts
            .iter()
            .map(|b| Pubkey::new_from_array(*b))
            .collect::<Vec<_>>();
        let accounts = client
            .get_multiple_accounts(&keys)
            .map_err(|e| format!("fetch nonce accounts: {e}"))?;

        let mut values = vec![[0u8; 32]; keys.len()];
        for (i, account) in accounts.iter().enumerate() {
            let account = account
                .as_ref()
                .ok_or_else(|| format!("nonce account {} does not exist", keys[i]))?;
            let state = parse_nonce(&account.data)
                .ok_or_else(|| format!("account {} is not a durable nonce", keys[i]))?;
            if state.authority != *expected_authority {
                return Err(format!(
                    "nonce account {} is controlled by {}, not the buying wallet {expected_authority}",
                    keys[i], state.authority
                ));
            }
            values[i] = state.value;
        }
        self.values.store(Arc::new(values));
        Ok(())
    }

    pub fn spawn_refresher(
        self: &Arc<Self>,
        rpc_url: String,
        interval_ms: u64,
        exit: Arc<AtomicBool>,
    ) -> JoinHandle<()> {
        let me = self.clone();
        Builder::new()
            .name("snipeNonce".to_string())
            .spawn(move || {
                let client =
                    RpcClient::new_with_commitment(rpc_url, CommitmentConfig::confirmed());
                let keys = me
                    .accounts
                    .iter()
                    .map(|b| Pubkey::new_from_array(*b))
                    .collect::<Vec<_>>();
                while !exit.load(Ordering::Relaxed) {
                    match client.get_multiple_accounts(&keys) {
                        Ok(accounts) => {
                            let mut values = vec![[0u8; 32]; keys.len()];
                            for (i, account) in accounts.iter().enumerate() {
                                if let Some(state) = account.as_ref().and_then(|a| parse_nonce(&a.data))
                                {
                                    values[i] = state.value;
                                }
                            }
                            me.values.store(Arc::new(values));
                        }
                        Err(e) => warn!("nonce refresh failed: {e}"),
                    }
                    std::thread::sleep(Duration::from_millis(interval_ms));
                }
            })
            .expect("spawn nonce refresher")
    }
}

pub struct NonceState {
    pub authority: Pubkey,
    pub value: [u8; 32],
}

/// `version(4) state(4) authority(32) durable_nonce(32) fee_calculator(8)`
pub fn parse_nonce(data: &[u8]) -> Option<NonceState> {
    if data.len() < 80 {
        return None;
    }
    // state 1 == Initialized
    if u32::from_le_bytes(data[4..8].try_into().ok()?) != 1 {
        return None;
    }
    Some(NonceState {
        authority: Pubkey::new_from_array(data[8..40].try_into().ok()?),
        value: data[40..72].try_into().ok()?,
    })
}

/// Latest blockhash, refreshed in the background at `confirmed`.
///
/// Replaces the single durable nonce the node bot used: with one nonce account, the second
/// launch inside a window reuses a nonce the first launch already advanced, so every later
/// transaction is rejected by every provider.
#[derive(Clone)]
pub struct BlockhashCache {
    inner: Arc<ArcSwap<[u8; 32]>>,
}

impl Default for BlockhashCache {
    fn default() -> Self {
        Self::new()
    }
}

impl BlockhashCache {
    pub fn new() -> Self {
        Self {
            inner: Arc::new(ArcSwap::from_pointee([0u8; 32])),
        }
    }

    #[inline(always)]
    pub fn get(&self) -> [u8; 32] {
        **self.inner.load()
    }

    pub fn is_ready(&self) -> bool {
        self.get() != [0u8; 32]
    }

    pub fn spawn_refresher(
        &self,
        rpc_url: String,
        interval_ms: u64,
        exit: Arc<AtomicBool>,
    ) -> JoinHandle<()> {
        let inner = self.inner.clone();
        Builder::new()
            .name("snipeBlockhash".to_string())
            .spawn(move || {
                let client =
                    RpcClient::new_with_commitment(rpc_url, CommitmentConfig::confirmed());
                while !exit.load(Ordering::Relaxed) {
                    match client.get_latest_blockhash() {
                        Ok(hash) => {
                            inner.store(Arc::new(hash.to_bytes()));
                            debug!("blockhash refreshed");
                        }
                        Err(e) => warn!("blockhash refresh failed: {e}"),
                    }
                    std::thread::sleep(Duration::from_millis(interval_ms));
                }
            })
            .expect("spawn blockhash refresher")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_truncated_global_account() {
        assert!(parse_global(&[0u8; 40]).is_err());
    }

    #[test]
    fn reads_fee_recipient_at_the_documented_offset() {
        let mut data = vec![0u8; 1045];
        data[8 + 1 + 32..8 + 1 + 32 + 32].copy_from_slice(&[7u8; 32]);
        let g = parse_global(&data).unwrap();
        assert_eq!(g.fee_recipient, Pubkey::new_from_array([7u8; 32]));
        assert_eq!(g.buyback_fee_recipients.len(), 8);
    }
}
