//! Position tracking and the three run modes.
//!
//! * **sync** (default on) — one token at a time. After firing, nothing else is bought until
//!   that position closes or the buy is known to have failed. The gate is a single atomic on
//!   the hot path; everything that decides when to lift it runs on a background thread.
//! * **test** (default off) — stop after one completed round trip, a buy that landed and a
//!   sell that closed the account. For proving a config end to end without letting it run.
//! * **ghost** (default off) — never touch the chain. The buy is priced, recorded and exited
//!   on paper against the real curve, so a strategy can be measured without spending.
//!
//! Position state comes from the chain rather than from the seller: the token account is
//! created by the buy and closed by the sell, so its lifecycle *is* the position's. That
//! keeps the gate correct even if the selling process restarts or is not running at all.

use std::{
    sync::{
        atomic::{AtomicBool, AtomicI64, AtomicU64, Ordering},
        Arc,
    },
    thread::{Builder, JoinHandle},
    time::{Duration, Instant},
};

use crossbeam_channel::{Receiver, Sender};
use log::{info, warn};
use solana_client::{
    rpc_client::{GetConfirmedSignaturesForAddress2Config, RpcClient},
    rpc_response::RpcConfirmedTransactionStatusWithSignature,
};
use solana_sdk::{commitment_config::CommitmentConfig, pubkey::Pubkey};

use crate::pumpfun::CurveParams;

/// What the hot path hands over after firing.
#[derive(Debug, Clone, Copy)]
pub struct OpenPosition {
    pub mint: Pubkey,
    /// slot the create was in, so the landed slot can be compared against it
    pub create_slot: u64,
    pub bonding_curve: Pubkey,
    pub token_account: Pubkey,
    /// tokens requested
    pub amount: u64,
    /// lamports the buy was allowed to spend, fees included
    pub cost: u64,
    pub ghost: bool,
}

#[derive(Debug, Default)]
pub struct PositionMetrics {
    pub opened: AtomicU64,
    pub closed: AtomicU64,
    /// buys that never appeared on chain
    pub never_landed: AtomicU64,
    /// launches skipped because a position was already open
    pub skipped_busy: AtomicU64,
    /// launches skipped because test mode has already completed its round trip
    pub skipped_disarmed: AtomicU64,
    /// buys that landed in the same slot as the create
    pub landed_same_slot: AtomicU64,
    /// landed one slot later
    pub landed_plus_one: AtomicU64,
    /// landed two or more slots later
    pub landed_later: AtomicU64,
    /// buys that landed but whose slot could not be read back
    pub landing_slot_unknown: AtomicU64,
    /// paper profit and loss, lamports, ghost mode only
    pub ghost_pnl_lamports: AtomicI64,
    pub ghost_wins: AtomicU64,
    pub ghost_losses: AtomicU64,
}

#[derive(Debug, Clone, Copy)]
pub struct ModeConfig {
    pub sync_mode: bool,
    pub test_mode: bool,
    pub ghost_mode: bool,
    /// how long a ghost position is held before it is priced out
    pub hold_ms: u64,
    /// how often the chain is polled while a position is open
    pub poll_ms: u64,
    /// after this long with no token account, the buy is treated as lost
    pub buy_timeout_ms: u64,
}

impl Default for ModeConfig {
    fn default() -> Self {
        Self {
            sync_mode: true,
            test_mode: false,
            ghost_mode: false,
            hold_ms: 1600,
            poll_ms: 200,
            buy_timeout_ms: 30_000,
        }
    }
}

/// The hot-path side of the gate: two atomics, nothing else.
pub struct PositionGate {
    busy: AtomicBool,
    armed: AtomicBool,
    sync_mode: bool,
    pub metrics: Arc<PositionMetrics>,
    tx: Sender<OpenPosition>,
}

impl PositionGate {
    /// True when a launch may be fired on.
    #[inline(always)]
    pub fn may_fire(&self) -> bool {
        if !self.armed.load(Ordering::Relaxed) {
            self.metrics.skipped_disarmed.fetch_add(1, Ordering::Relaxed);
            return false;
        }
        if self.sync_mode && self.busy.load(Ordering::Relaxed) {
            self.metrics.skipped_busy.fetch_add(1, Ordering::Relaxed);
            return false;
        }
        true
    }

    /// Claims the slot. Returns false if another launch won the race, which can only happen
    /// when several detect threads are running.
    #[inline(always)]
    pub fn claim(&self) -> bool {
        if !self.sync_mode {
            return true;
        }
        self.busy
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Relaxed)
            .is_ok()
    }

    /// Hands the position to the tracker. Never blocks.
    #[inline(always)]
    pub fn opened(&self, position: OpenPosition) {
        self.metrics.opened.fetch_add(1, Ordering::Relaxed);
        if self.tx.try_send(position).is_err() {
            // the tracker is the only thing that can clear the gate, so a full queue would
            // wedge the sniper: release rather than hold a slot nobody will free
            self.busy.store(false, Ordering::Release);
        }
    }

    pub fn is_busy(&self) -> bool {
        self.busy.load(Ordering::Relaxed)
    }

    pub fn is_armed(&self) -> bool {
        self.armed.load(Ordering::Relaxed)
    }
}

/// Starts the tracker and returns the gate the hot path holds.
pub fn start(
    rpc_url: String,
    modes: ModeConfig,
    curve: CurveParams,
    exit: Arc<AtomicBool>,
) -> (Arc<PositionGate>, JoinHandle<()>) {
    let (tx, rx) = crossbeam_channel::bounded::<OpenPosition>(64);
    let metrics = Arc::new(PositionMetrics::default());
    let gate = Arc::new(PositionGate {
        busy: AtomicBool::new(false),
        armed: AtomicBool::new(true),
        sync_mode: modes.sync_mode,
        metrics: metrics.clone(),
        tx,
    });

    let handle = {
        let gate = gate.clone();
        Builder::new()
            .name("snipePositions".to_string())
            .spawn(move || track(rpc_url, modes, curve, rx, gate, metrics, exit))
            .expect("spawn position tracker")
    };
    (gate, handle)
}

fn track(
    rpc_url: String,
    modes: ModeConfig,
    curve: CurveParams,
    rx: Receiver<OpenPosition>,
    gate: Arc<PositionGate>,
    metrics: Arc<PositionMetrics>,
    exit: Arc<AtomicBool>,
) {
    let client = RpcClient::new_with_commitment(rpc_url.clone(), CommitmentConfig::confirmed());

    while !exit.load(Ordering::Relaxed) {
        let Ok(position) = rx.recv_timeout(Duration::from_millis(250)) else {
            continue;
        };

        if position.ghost {
            ghost_cycle(&client, &modes, &curve, &position, &metrics);
        } else {
            live_cycle(&client, &rpc_url, &modes, &position, &metrics);
        }

        metrics.closed.fetch_add(1, Ordering::Relaxed);
        if modes.test_mode {
            gate.armed.store(false, Ordering::Release);
            info!("test mode: one round trip complete, no further buys");
        }
        gate.busy.store(false, Ordering::Release);
    }
}

/// Waits for the buy to appear, then for the sell to close the account.
fn live_cycle(
    client: &RpcClient,
    rpc_url: &str,
    modes: &ModeConfig,
    position: &OpenPosition,
    metrics: &Arc<PositionMetrics>,
) {
    let poll = Duration::from_millis(modes.poll_ms);
    let deadline = Instant::now() + Duration::from_millis(modes.buy_timeout_ms);
    let mut landed = false;

    loop {
        let exists = client
            .get_account_with_commitment(&position.token_account, CommitmentConfig::processed())
            .map(|r| r.value.is_some())
            .unwrap_or(false);


        if exists {
            if !landed {
                landed = true;
                // off this thread: reading the landing slot has to wait for `confirmed`, and
                // this thread is the one holding the position gate. A gate held for an extra
                // second or two is a launch not seen, which is exactly the cost the number
                // being measured is supposed to expose.
                spawn_landing_record(rpc_url, *position, metrics.clone());
            }
        } else if landed {
            info!("position {} closed", position.mint);
            return;
        } else if Instant::now() > deadline {
            metrics.never_landed.fetch_add(1, Ordering::Relaxed);
            info!("position {} never landed, releasing", position.mint);
            return;
        }
        std::thread::sleep(poll);
    }
}

/// Prices the position out against the real curve without touching it.
fn ghost_cycle(
    client: &RpcClient,
    modes: &ModeConfig,
    curve: &CurveParams,
    position: &OpenPosition,
    metrics: &PositionMetrics,
) {
    std::thread::sleep(Duration::from_millis(modes.hold_ms));

    let proceeds = match client.get_account_data(&position.bonding_curve) {
        Ok(data) => match read_reserves(&data) {
            Some((virtual_token, virtual_sol)) => {
                sell_proceeds(curve, virtual_sol, virtual_token, position.amount)
            }
            None => {
                warn!("ghost: bonding curve {} unreadable", position.bonding_curve);
                return;
            }
        },
        Err(e) => {
            warn!("ghost: could not read {}: {e}", position.bonding_curve);
            return;
        }
    };

    let pnl = proceeds as i64 - position.cost as i64;
    metrics.ghost_pnl_lamports.fetch_add(pnl, Ordering::Relaxed);
    if pnl >= 0 {
        metrics.ghost_wins.fetch_add(1, Ordering::Relaxed);
    } else {
        metrics.ghost_losses.fetch_add(1, Ordering::Relaxed);
    }
    info!(
        "ghost {}: in {} lamports, out {} lamports, pnl {} ({} wins / {} losses, total {})",
        position.mint,
        position.cost,
        proceeds,
        pnl,
        metrics.ghost_wins.load(Ordering::Relaxed),
        metrics.ghost_losses.load(Ordering::Relaxed),
        metrics.ghost_pnl_lamports.load(Ordering::Relaxed),
    );
}

/// How long to wait for the landing slot to become readable.
///
/// The token account is first seen at `processed`, and `getSignaturesForAddress` refuses to
/// answer below `confirmed`, which is a slot or two behind. Long enough to cover that gap,
/// short enough that a lookup that never resolves does not sit on the position gate.
const LANDING_LOOKUP_ATTEMPTS: usize = 12;
const LANDING_LOOKUP_INTERVAL: Duration = Duration::from_millis(250);

/// Picks the transaction that created the account: the oldest one that did not fail.
///
/// `getSignaturesForAddress` returns newest first, and by the time the sell runs there are
/// two signatures on this account. A failed transaction never created anything, so it cannot
/// be the one, which also keeps a rejected duplicate from being mistaken for the buy.
fn creating_signature(
    signatures: &[RpcConfirmedTransactionStatusWithSignature],
) -> Option<(u64, &str)> {
    signatures
        .iter()
        .filter(|s| s.err.is_none())
        .min_by_key(|s| s.slot)
        .map(|s| (s.slot, s.signature.as_str()))
}

/// The slot the buy actually landed in, read off the transaction that created the token
/// account.
///
/// This used to be `client.get_slot()`, which returns the slot the *poller* happens to be in
/// when it first notices the account — one or two slots late, given a 200ms poll interval on
/// top of an RPC round trip. That number could never report a 0 even when the buy landed in
/// the create's own slot, which made it useless for the single question it exists to answer.
fn read_landed_slot(client: &RpcClient, token_account: &Pubkey) -> Option<(u64, String)> {
    for attempt in 0..LANDING_LOOKUP_ATTEMPTS {
        if attempt > 0 {
            std::thread::sleep(LANDING_LOOKUP_INTERVAL);
        }
        let config = GetConfirmedSignaturesForAddress2Config {
            limit: Some(10),
            commitment: Some(CommitmentConfig::confirmed()),
            ..Default::default()
        };
        let Ok(signatures) = client.get_signatures_for_address_with_config(token_account, config)
        else {
            continue;
        };
        if let Some((slot, signature)) = creating_signature(&signatures) {
            return Some((slot, signature.to_string()));
        }
    }
    None
}

/// Reads and records the landing slot on its own thread.
///
/// One short-lived thread per landed position, which is a handful a minute. The alternative
/// is doing the lookup inline on the tracker thread, which holds the position gate.
fn spawn_landing_record(rpc_url: &str, position: OpenPosition, metrics: Arc<PositionMetrics>) {
    let rpc_url = rpc_url.to_string();
    if let Err(e) = Builder::new()
        .name("snipeLanding".to_string())
        .spawn(move || {
            let client =
                RpcClient::new_with_commitment(rpc_url, CommitmentConfig::confirmed());
            record_landing(&client, &position, &metrics);
        })
    {
        warn!("could not spawn landing recorder: {e}");
    }
}

/// Records how many slots behind the create our buy landed.
///
/// This is the number that says whether the sniper is actually competitive. Same slot means
/// the buy reached the leader that was already building the block containing the create;
/// anything later means the race was lost on the network, not in this process.
///
/// A landing whose slot cannot be read is counted separately rather than folded into
/// `landed_later` — a measurement that fails is not the same as a race that was lost, and
/// quietly conflating the two is what made the old number worthless.
fn record_landing(client: &RpcClient, position: &OpenPosition, metrics: &PositionMetrics) {
    let Some((landed_slot, signature)) = read_landed_slot(client, &position.token_account) else {
        metrics.landing_slot_unknown.fetch_add(1, Ordering::Relaxed);
        warn!(
            "{}: buy landed but its slot could not be read within {}ms, not counted",
            position.mint,
            LANDING_LOOKUP_ATTEMPTS as u64 * LANDING_LOOKUP_INTERVAL.as_millis() as u64,
        );
        return;
    };
    let delta = landed_slot.saturating_sub(position.create_slot);
    match delta {
        0 => metrics.landed_same_slot.fetch_add(1, Ordering::Relaxed),
        1 => metrics.landed_plus_one.fetch_add(1, Ordering::Relaxed),
        _ => metrics.landed_later.fetch_add(1, Ordering::Relaxed),
    };
    info!(
        "{} create slot {} -> landed slot {} (+{delta}) sig {signature} \
         | same {} / +1 {} / later {} / unreadable {}",
        position.mint,
        position.create_slot,
        landed_slot,
        metrics.landed_same_slot.load(Ordering::Relaxed),
        metrics.landed_plus_one.load(Ordering::Relaxed),
        metrics.landed_later.load(Ordering::Relaxed),
        metrics.landing_slot_unknown.load(Ordering::Relaxed),
    );
}

/// `BondingCurve`: discriminator(8) virtual_token_reserves(8) virtual_quote_reserves(8)
pub fn read_reserves(data: &[u8]) -> Option<(u64, u64)> {
    let token = u64::from_le_bytes(data.get(8..16)?.try_into().ok()?);
    let sol = u64::from_le_bytes(data.get(16..24)?.try_into().ok()?);
    Some((token, sol))
}

/// Lamports out for selling `amount` tokens into the curve, fees deducted.
///
/// Constant product in the other direction: putting tokens in moves the price down, so the
/// proceeds are the difference in the sol side of the invariant.
pub fn sell_proceeds(
    curve: &CurveParams,
    virtual_sol: u64,
    virtual_token: u64,
    amount: u64,
) -> u64 {
    if amount == 0 || virtual_token == 0 || virtual_sol == 0 {
        return 0;
    }
    let k = virtual_sol as u128 * virtual_token as u128;
    let new_token = virtual_token as u128 + amount as u128;
    let new_sol = k / new_token;
    let gross = (virtual_sol as u128).saturating_sub(new_sol);
    let fee = gross * curve.total_fee_bps() as u128 / 10_000;
    gross.saturating_sub(fee) as u64
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sig(signature: &str, slot: u64, failed: bool) -> RpcConfirmedTransactionStatusWithSignature {
        RpcConfirmedTransactionStatusWithSignature {
            signature: signature.to_string(),
            slot,
            err: failed.then(|| {
                solana_sdk::transaction::TransactionError::InstructionError(
                    0,
                    solana_sdk::instruction::InstructionError::Custom(1),
                )
            }),
            memo: None,
            block_time: None,
            confirmation_status: None,
        }
    }

    /// The landing slot is the whole point of the metric, so the rule that picks which
    /// signature it comes from has to be exactly right: oldest, and never a failed one.
    #[test]
    fn the_creating_transaction_is_the_oldest_successful_one() {
        // what the RPC actually returns: newest first, buy and sell both present
        let history = [sig("sell", 310, false), sig("buy", 300, false)];
        assert_eq!(creating_signature(&history), Some((300, "buy")));

        // a rejected duplicate landing earlier must not be mistaken for the buy
        let with_failure = [
            sig("sell", 310, false),
            sig("buy", 300, false),
            sig("rejected", 299, true),
        ];
        assert_eq!(creating_signature(&with_failure), Some((300, "buy")));

        // nothing usable is None, not a guess -- the caller counts that separately
        assert_eq!(creating_signature(&[]), None);
        assert_eq!(creating_signature(&[sig("rejected", 299, true)]), None);
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

    #[test]
    fn selling_back_immediately_loses_exactly_the_fees() {
        let c = curve();
        let spend = 1_000_000_000u64;
        let plan = c.plan_buy(0, spend, 0, 0);

        // the curve after our own buy
        let vsol = c.initial_virtual_sol_reserves + spend;
        let vtok = c.initial_virtual_token_reserves - plan.amount;
        let out = sell_proceeds(&c, vsol, vtok, plan.amount);

        assert!(out < spend, "a round trip cannot profit at a flat price");
        // two fee legs, buy and sell, so ~2% on a 100bps total
        let loss_bps = (spend - out) * 10_000 / spend;
        assert!(
            (100..=400).contains(&loss_bps),
            "round trip loss of {loss_bps}bps is not fee shaped"
        );
    }

    #[test]
    fn proceeds_rise_with_the_curve() {
        let c = curve();
        let amount = 1_000_000_000_000u64;
        let flat = sell_proceeds(&c, 30_000_000_000, 1_073_000_000_000_000, amount);
        let pumped = sell_proceeds(&c, 60_000_000_000, 1_073_000_000_000_000, amount);
        assert!(pumped > flat, "a higher sol reserve must pay more");
    }

    #[test]
    fn degenerate_inputs_are_zero_rather_than_a_panic() {
        let c = curve();
        assert_eq!(sell_proceeds(&c, 0, 1, 1), 0);
        assert_eq!(sell_proceeds(&c, 1, 0, 1), 0);
        assert_eq!(sell_proceeds(&c, 1, 1, 0), 0);
    }

    #[test]
    fn reserves_are_read_from_the_documented_offsets() {
        let mut data = vec![0u8; 64];
        data[8..16].copy_from_slice(&7u64.to_le_bytes());
        data[16..24].copy_from_slice(&9u64.to_le_bytes());
        assert_eq!(read_reserves(&data), Some((7, 9)));
        assert_eq!(read_reserves(&[0u8; 4]), None);
    }

    #[test]
    fn the_gate_blocks_a_second_launch_in_sync_mode() {
        let exit = Arc::new(AtomicBool::new(true));
        let (gate, handle) = start(
            "http://127.0.0.1:1".to_string(),
            ModeConfig {
                sync_mode: true,
                ..ModeConfig::default()
            },
            curve(),
            exit.clone(),
        );

        assert!(gate.may_fire());
        assert!(gate.claim(), "first launch claims the slot");
        assert!(!gate.claim(), "second launch cannot claim it");
        assert!(!gate.may_fire(), "and is told not to fire");
        assert_eq!(gate.metrics.skipped_busy.load(Ordering::Relaxed), 1);
        let _ = handle.join();
    }

    #[test]
    fn parallel_mode_never_blocks() {
        let exit = Arc::new(AtomicBool::new(true));
        let (gate, handle) = start(
            "http://127.0.0.1:1".to_string(),
            ModeConfig {
                sync_mode: false,
                ..ModeConfig::default()
            },
            curve(),
            exit.clone(),
        );
        assert!(gate.claim());
        assert!(gate.claim(), "parallel mode allows overlapping positions");
        assert!(gate.may_fire());
        let _ = handle.join();
    }
}
