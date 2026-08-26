//! pump.fun launch detection and address derivation.
//!
//! Detection runs on *reconstructed entries*, never on raw shred payloads: a pubkey can
//! straddle two shreds and can live only in coding shreds, so scanning shred bytes
//! false-negatives on real launches.
//!
//! A launch is identified by (pump program id + create instruction discriminator). Both the
//! classic `create` (spl-token + metaplex metadata) and the current `create_v2` (token-2022,
//! no metadata account, mayhem accounts) are accepted. Nothing depends on an exact account
//! set, so accounts appended by future pump.fun upgrades cannot silently drop launches.

use std::sync::atomic::{AtomicU64, Ordering};

use solana_sdk::{
    instruction::CompiledInstruction, pubkey::Pubkey, transaction::VersionedTransaction,
};

/// Creates whose accounts could not be read out of the transaction's static keys.
///
/// Every account the parse needs lives in the static key list today, so this stays at zero.
/// If a pump.fun upgrade moves any of them behind an address lookup table the parse starts
/// returning `None` and those launches become invisible -- with no other signal that
/// anything changed. This counter is that signal, which is why the discriminator match and
/// the account read are counted separately.
pub static UNRESOLVED_CREATES: AtomicU64 = AtomicU64::new(0);

/// pump.fun bonding-curve program
pub const PUMP_PROGRAM: Pubkey =
    Pubkey::from_str_const("6EF8rrecthR5Dkzon8Nwu78hRvfCKubJ14M5uBEwF6P");
/// classic spl-token
pub const TOKEN_PROGRAM: Pubkey =
    Pubkey::from_str_const("TokenkegQfeZyiNwAJbNbGKPFXCWuBvf9Ss623VQ5DA");
/// token-2022, used by every `create_v2` launch
pub const TOKEN_2022_PROGRAM: Pubkey =
    Pubkey::from_str_const("TokenzQdBNbLqP5VEhdkAS6EPFLC1PHnBqCXEpPxuEb");
pub const ASSOCIATED_TOKEN_PROGRAM: Pubkey =
    Pubkey::from_str_const("ATokenGPvbdGVxr1b2hvZbsiqW5xWH25efTNsLJA8knL");
pub const COMPUTE_BUDGET_PROGRAM: Pubkey =
    Pubkey::from_str_const("ComputeBudget111111111111111111111111111111");
pub const FEE_PROGRAM: Pubkey =
    Pubkey::from_str_const("pfeeUxB6jkeY1Hxd7CsFCAjcbHA9rWtchMGdZ6VojVZ");

// PDAs with constant seeds, precomputed so the hot path never derives them.
/// `["global"]`
pub const GLOBAL: Pubkey = Pubkey::from_str_const("4wTV1YmiEkRvAtNtsSGPtUrqRYQMe5SKy2uB4Jjaxnjf");
/// `["__event_authority"]`
pub const EVENT_AUTHORITY: Pubkey =
    Pubkey::from_str_const("Ce6TQqeHC9p8KetsN6JsjHK7UTZk7nasjjnr7XxXp9F1");
/// `["global_volume_accumulator"]`
pub const GLOBAL_VOLUME_ACCUMULATOR: Pubkey =
    Pubkey::from_str_const("Hq2wp8uJ9jCPsYgNHex8RtqdvMPfVGoYwjvF1ATiwn2Y");
/// `["fee_config", pump program id]`, owned by the fee program
pub const FEE_CONFIG: Pubkey =
    Pubkey::from_str_const("8Wf5TiAheLUqBrKXeYg2JtAFFMWtKdG2BSFgqUcPVwTt");

// anchor discriminators (first 8 bytes of instruction data)
pub const DISC_CREATE: [u8; 8] = [24, 30, 200, 40, 5, 28, 7, 119];
pub const DISC_CREATE_V2: [u8; 8] = [214, 144, 76, 236, 95, 139, 49, 180];
pub const DISC_BUY: [u8; 8] = [102, 6, 61, 18, 1, 218, 235, 234];
pub const DISC_BUY_V2: [u8; 8] = [184, 23, 238, 97, 103, 197, 211, 61];
pub const DISC_BUY_EXACT_SOL_IN: [u8; 8] = [56, 252, 116, 8, 158, 223, 205, 95];

pub const SEED_BONDING_CURVE: &[u8] = b"bonding-curve";
pub const SEED_BONDING_CURVE_V2: &[u8] = b"bonding-curve-v2";
pub const SEED_CREATOR_VAULT: &[u8] = b"creator-vault";
pub const SEED_USER_VOLUME_ACCUMULATOR: &[u8] = b"user_volume_accumulator";

/// Everything the hot path needs about a launch, with no re-serialization.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PumpCreateInfo {
    pub mint: Pubkey,
    pub bonding_curve: Pubkey,
    pub associated_bonding_curve: Pubkey,
    /// `creator` instruction argument; seed of the creator-vault PDA
    pub creator: Pubkey,
    /// fee payer of the create transaction
    pub user: Pubkey,
    pub token_program: Pubkey,
    /// `max_sol_cost` of the dev buy in the same transaction (0 when there is none)
    pub dev_buy_lamports: u64,
    pub is_v2: bool,
}

#[inline(always)]
fn disc(data: &[u8]) -> Option<&[u8; 8]> {
    data.get(..8)?.try_into().ok()
}

/// A pump.fun buy landing in a block, as seen off the shred stream. This is what the v1.1
/// confirmation trigger counts: buys landing in the create block after the dev's initial buy.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PumpBuyInfo {
    pub mint: Pubkey,
    /// fee payer of the buy transaction
    pub buyer: Pubkey,
    /// SOL committed to the buy, in lamports. For `buy`/`buy_v2` this is `max_sol_cost`
    /// (fee included); for `buy_exact_sol_in` it is the exact sol argument. It is the amount
    /// pledged, which is what is visible pre-execution and what the trigger counts.
    pub sol_lamports: u64,
    /// First 8 bytes of the transaction's first signature. The early-detect path re-emits the
    /// same buy on every subsequent shred of a segment, so the confirm trigger dedups on this
    /// - without it one confirming buy is counted many times and the trigger fires off one buy.
    pub sig8: [u8; 8],
}

/// Returns the first pump.fun buy in `tx`, if any. Skips a buy whose signer is the mint's own
/// creator in the same transaction (that is the dev buy, handled by `parse_create`).
///
/// Hot path: no allocation, no formatting, no logging.
#[inline]
pub fn parse_buy(tx: &VersionedTransaction) -> Option<PumpBuyInfo> {
    let keys = tx.message.static_account_keys();
    let instructions: &[CompiledInstruction] = match &tx.message {
        solana_sdk::message::VersionedMessage::V0(v0) => &v0.instructions,
        solana_sdk::message::VersionedMessage::Legacy(l) => &l.instructions,
    };
    // a transaction that also carries a create is a dev buy, not a confirming buy
    for ix in instructions {
        if keys.get(ix.program_id_index as usize) == Some(&PUMP_PROGRAM) {
            if let Some(d) = disc(&ix.data) {
                if *d == DISC_CREATE || *d == DISC_CREATE_V2 {
                    return None;
                }
            }
        }
    }
    for ix in instructions {
        if keys.get(ix.program_id_index as usize) != Some(&PUMP_PROGRAM) {
            continue;
        }
        let Some(d) = disc(&ix.data) else { continue };
        let sol = match *d {
            // (amount_tokens: u64, max_sol_cost: u64)
            DISC_BUY | DISC_BUY_V2 => {
                u64::from_le_bytes(ix.data.get(16..24)?.try_into().ok()?)
            }
            // (sol_in: u64, min_tokens_out: u64)
            DISC_BUY_EXACT_SOL_IN => {
                u64::from_le_bytes(ix.data.get(8..16)?.try_into().ok()?)
            }
            _ => continue,
        };
        // pump buy accounts: 0 global, 1 fee_recipient, 2 mint, 3 bonding_curve, ...
        let mint = *key_at(keys, ix, 2)?;
        // the fee payer is the buyer on a standalone buy transaction
        let buyer = *keys.first()?;
        let mut sig8 = [0u8; 8];
        if let Some(s) = tx.signatures.first() {
            sig8.copy_from_slice(&s.as_ref()[..8]);
        }
        return Some(PumpBuyInfo { mint, buyer, sol_lamports: sol, sig8 });
    }
    None
}

/// Returns the launch described by `tx`, if it is a pump.fun create.
///
/// Hot path: no allocation, no formatting, no logging.
#[inline]
pub fn parse_create(tx: &VersionedTransaction) -> Option<PumpCreateInfo> {
    let keys = tx.message.static_account_keys();
    let instructions: &[CompiledInstruction] = match &tx.message {
        solana_sdk::message::VersionedMessage::V0(v0) => &v0.instructions,
        solana_sdk::message::VersionedMessage::Legacy(l) => &l.instructions,
    };

    let mut info: Option<PumpCreateInfo> = None;
    let mut dev_buy_lamports = 0u64;

    for ix in instructions {
        // program id must resolve inside the static keys; pump creates never come from a LUT
        if keys.get(ix.program_id_index as usize) != Some(&PUMP_PROGRAM) {
            continue;
        }
        let Some(d) = disc(&ix.data) else { continue };

        match *d {
            DISC_CREATE_V2 => match read_create_v2(keys, ix) {
                Some(i) => info = Some(i),
                None => {
                    UNRESOLVED_CREATES.fetch_add(1, Ordering::Relaxed);
                    return None;
                }
            },
            DISC_CREATE => match read_create(keys, ix) {
                Some(i) => info = Some(i),
                None => {
                    UNRESOLVED_CREATES.fetch_add(1, Ordering::Relaxed);
                    return None;
                }
            },
            // dev buy in the same transaction; `max_sol_cost` is the second u64 argument
            DISC_BUY | DISC_BUY_V2 => {
                if let Some(b) = ix.data.get(16..24) {
                    dev_buy_lamports = u64::from_le_bytes(b.try_into().ok()?);
                }
            }
            // buy_exact_sol_in: the first u64 argument is the sol amount
            DISC_BUY_EXACT_SOL_IN => {
                if let Some(b) = ix.data.get(8..16) {
                    dev_buy_lamports = u64::from_le_bytes(b.try_into().ok()?);
                }
            }
            _ => {}
        }
    }

    let mut info = info?;
    info.dev_buy_lamports = dev_buy_lamports;
    Some(info)
}

/// accounts: 0 mint, 1 mint_authority, 2 bonding_curve, 3 associated_bonding_curve,
///           4 global, 5 user, 6 system, 7 token_2022, ...
/// args: name:String, symbol:String, uri:String, creator:Pubkey,
///       is_mayhem_mode:bool, is_cashback_enabled:OptionBool
#[inline(always)]
fn read_create_v2(keys: &[Pubkey], ix: &CompiledInstruction) -> Option<PumpCreateInfo> {
    Some(PumpCreateInfo {
        mint: *key_at(keys, ix, 0)?,
        bonding_curve: *key_at(keys, ix, 2)?,
        associated_bonding_curve: *key_at(keys, ix, 3)?,
        creator: trailing_pubkey(&ix.data, 2)?,
        user: *key_at(keys, ix, 5)?,
        token_program: *key_at(keys, ix, 7).unwrap_or(&TOKEN_2022_PROGRAM),
        dev_buy_lamports: 0,
        is_v2: true,
    })
}

/// accounts: 0 mint, 1 mint_authority, 2 bonding_curve, 3 associated_bonding_curve,
///           4 global, 5 mpl, 6 metadata, 7 user, 8 system, 9 token_program, ...
/// args: name:String, symbol:String, uri:String, creator:Pubkey
#[inline(always)]
fn read_create(keys: &[Pubkey], ix: &CompiledInstruction) -> Option<PumpCreateInfo> {
    Some(PumpCreateInfo {
        mint: *key_at(keys, ix, 0)?,
        bonding_curve: *key_at(keys, ix, 2)?,
        associated_bonding_curve: *key_at(keys, ix, 3)?,
        creator: trailing_pubkey(&ix.data, 0)?,
        user: *key_at(keys, ix, 7)?,
        token_program: *key_at(keys, ix, 9).unwrap_or(&TOKEN_PROGRAM),
        dev_buy_lamports: 0,
        is_v2: false,
    })
}

#[inline(always)]
fn key_at<'a>(keys: &'a [Pubkey], ix: &CompiledInstruction, pos: usize) -> Option<&'a Pubkey> {
    keys.get(*ix.accounts.get(pos)? as usize)
}

/// Reads the `creator` pubkey argument, which sits `tail_bytes` before the end of the
/// instruction data (create: nothing after it, create_v2: is_mayhem_mode + OptionBool).
#[inline(always)]
fn trailing_pubkey(data: &[u8], tail_bytes: usize) -> Option<Pubkey> {
    let end = data.len().checked_sub(tail_bytes)?;
    let start = end.checked_sub(32)?;
    if start < 8 {
        return None;
    }
    let bytes: [u8; 32] = data.get(start..end)?.try_into().ok()?;
    Some(Pubkey::new_from_array(bytes))
}

// ---------------------------------------------------------------------------
// address derivation
// ---------------------------------------------------------------------------

#[inline]
pub fn creator_vault(creator: &Pubkey) -> Pubkey {
    Pubkey::find_program_address(&[SEED_CREATOR_VAULT, creator.as_ref()], &PUMP_PROGRAM).0
}

#[inline]
pub fn bonding_curve_v2(mint: &Pubkey) -> Pubkey {
    Pubkey::find_program_address(&[SEED_BONDING_CURVE_V2, mint.as_ref()], &PUMP_PROGRAM).0
}

#[inline]
pub fn bonding_curve(mint: &Pubkey) -> Pubkey {
    Pubkey::find_program_address(&[SEED_BONDING_CURVE, mint.as_ref()], &PUMP_PROGRAM).0
}

#[inline]
pub fn user_volume_accumulator(user: &Pubkey) -> Pubkey {
    Pubkey::find_program_address(
        &[SEED_USER_VOLUME_ACCUMULATOR, user.as_ref()],
        &PUMP_PROGRAM,
    )
    .0
}

#[inline]
pub fn associated_token_address(owner: &Pubkey, token_program: &Pubkey, mint: &Pubkey) -> Pubkey {
    Pubkey::find_program_address(
        &[owner.as_ref(), token_program.as_ref(), mint.as_ref()],
        &ASSOCIATED_TOKEN_PROGRAM,
    )
    .0
}

// ---------------------------------------------------------------------------
// bonding curve math
// ---------------------------------------------------------------------------

/// Initial virtual/real reserves and fee rates from the pump.fun global config.
#[derive(Debug, Clone, Copy)]
pub struct CurveParams {
    pub initial_virtual_sol_reserves: u64,
    pub initial_virtual_token_reserves: u64,
    pub initial_real_token_reserves: u64,
    pub fee_basis_points: u64,
    pub creator_fee_basis_points: u64,
}

/// What to put in the buy instruction's two u64 fields.
///
/// The two fields sit at the same byte offsets for both pump buy variants, so the same patch
/// mechanism serves both - only their MEANING differs by instruction:
///   * `buy`               -> arg0 = exact token amount, arg1 = max_sol_cost (spend cap)
///   * `buy_exact_sol_in`  -> arg0 = sol_in (fixed spend), arg1 = min_tokens_out (floor)
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BuyPlan {
    /// `buy`: exact token amount. `buy_exact_sol_in`: the sol to spend (= budget).
    pub amount: u64,
    /// `buy`: max_sol_cost cap. `buy_exact_sol_in`: min_tokens_out floor.
    pub max_sol_cost: u64,
    /// Instruction-INDEPENDENT cost basis: the budget the caller asked to spend, in
    /// lamports. Anything that accounts money (ghost PnL, the seller's stop-loss basis,
    /// tip sizing) must read this, never `max_sol_cost` — which is a TOKEN amount under
    /// `buy_exact_sol_in` and a slippage-padded cap under `buy`.
    pub budget_lamports: u64,
    /// Instruction-INDEPENDENT expected token fill at the priced depth. Under `buy` this
    /// equals `amount`; under `buy_exact_sol_in` it is the pre-floor estimate.
    pub expected_tokens: u64,
}

impl CurveParams {
    #[inline]
    pub fn total_fee_bps(&self) -> u64 {
        self.fee_basis_points + self.creator_fee_basis_points
    }

    /// Tokens received for `sol_in` lamports on a fresh curve, matching
    /// `GlobalAccount::getInitialBuyPrice` in pumpdotfun-sdk.
    #[inline]
    pub fn initial_buy_price(&self, sol_in: u64) -> u64 {
        if sol_in == 0 {
            return 0;
        }
        let n =
            self.initial_virtual_sol_reserves as u128 * self.initial_virtual_token_reserves as u128;
        let i = self.initial_virtual_sol_reserves as u128 + sol_in as u128;
        let r = n / i + 1;
        let s = (self.initial_virtual_token_reserves as u128).saturating_sub(r);
        s.min(self.initial_real_token_reserves as u128) as u64
    }

    /// Tokens we expect when buying `our_sol` of *curve* volume right behind a dev buy of
    /// `dev_sol`.
    #[inline]
    pub fn tokens_behind_dev_buy(&self, dev_sol: u64, our_sol: u64) -> u64 {
        let before = self.initial_buy_price(dev_sol);
        let after = self.initial_buy_price(dev_sol.saturating_add(our_sol));
        after.saturating_sub(before)
    }

    /// Sizes a buy so the *total* lamports leaving the wallet land on `budget_lamports`.
    ///
    /// pump.fun `buy` takes an exact token amount and a cap, so the slippage that matters is
    /// the error between what we ask for and what the curve actually charges. Two things
    /// caused that error before:
    ///
    /// * fees were ignored. Asking for the tokens that `budget` buys at curve price means
    ///   the program also charges ~1% on top, so the real spend overshot the budget and the
    ///   cap had to be padded by 15% to avoid `TooMuchSolRequired`.
    /// * the dev buy in the same transaction moves the curve first, so pricing off the
    ///   initial reserves overstates how many tokens the budget buys.
    ///
    /// Here the fee is divided out first, the dev buy is priced in, and `haircut_bps` is
    /// shaved off the token amount so a curve that moved slightly further than expected
    /// still fits under the cap. `slippage_bps` is headroom on the cap only — it is never
    /// spent unless the curve actually moved.
    #[inline]
    pub fn plan_buy(
        &self,
        dev_buy_lamports: u64,
        budget_lamports: u64,
        haircut_bps: u64,
        slippage_bps: u64,
    ) -> BuyPlan {
        // budget covers curve cost + fees, so back the fee out before pricing
        let curve_sol = (budget_lamports as u128 * 10_000
            / (10_000 + self.total_fee_bps()) as u128) as u64;
        let tokens = self.tokens_behind_dev_buy(dev_buy_lamports, curve_sol);
        let amount = (tokens as u128 * (10_000 - haircut_bps.min(10_000)) as u128 / 10_000) as u64;
        let max_sol_cost =
            (budget_lamports as u128 * (10_000 + slippage_bps) as u128 / 10_000) as u64;
        BuyPlan {
            amount,
            max_sol_cost,
            budget_lamports,
            expected_tokens: amount,
        }
    }

    /// Sizes a `buy_exact_sol_in`, the instruction the leader uses on ~75% of buys.
    ///
    /// Fixes the SOL spent (`sol_in` = the full budget) and sets a `min_tokens_out` floor. The
    /// spend can never overshoot, and unlike `buy` there is NO exact-token computation whose
    /// depth error blows the cap - the depth estimate only affects the floor. If the curve moved
    /// up (fewer tokens) below the floor it reverts (desired: we were overtaken); if it moved
    /// down (more tokens) it fills. `prior_flow_lamports` is the net SOL in the curve ahead of
    /// us; `slippage_bps` is how far below the expected token amount we still accept
    /// (E4Ez's median ~640 bps).
    #[inline]
    pub fn plan_exact_sol_in(
        &self,
        prior_flow_lamports: u64,
        budget_lamports: u64,
        slippage_bps: u64,
    ) -> BuyPlan {
        let curve_sol = (budget_lamports as u128 * 10_000
            / (10_000 + self.total_fee_bps()) as u128) as u64;
        let expected = self.tokens_behind_dev_buy(prior_flow_lamports, curve_sol);
        let min_tokens =
            (expected as u128 * (10_000 - slippage_bps.min(10_000)) as u128 / 10_000) as u64;
        BuyPlan {
            amount: budget_lamports, // sol_in: the whole budget, fixed
            max_sol_cost: min_tokens, // min_tokens_out floor
            budget_lamports,
            expected_tokens: expected,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Live mainnet values at the time of writing.
    fn params() -> CurveParams {
        CurveParams {
            initial_virtual_sol_reserves: 30_000_000_000,
            initial_virtual_token_reserves: 1_073_000_000_000_000,
            initial_real_token_reserves: 793_100_000_000_000,
            fee_basis_points: 95,
            creator_fee_basis_points: 5,
        }
    }

    /// What the program will actually charge for `amount` tokens, curve price plus fees.
    fn real_cost(p: &CurveParams, dev: u64, amount: u64) -> u64 {
        // invert the curve: find the lamports that buy `amount` tokens after the dev buy
        let mut lo = 0u64;
        let mut hi = 100_000_000_000u64;
        while lo < hi {
            let mid = (lo + hi) / 2;
            if p.tokens_behind_dev_buy(dev, mid) < amount {
                lo = mid + 1;
            } else {
                hi = mid;
            }
        }
        (lo as u128 * (10_000 + p.total_fee_bps()) as u128 / 10_000) as u64
    }

    /// Regression for the confirm-mode pricing bug: the buy lands on top of ALL the flow ahead
    /// (dev + confirming buys), so plan_buy must be priced against that full depth. Pricing
    /// against the dev buy alone under-prices the shallow curve and the exact-token request
    /// blows past max_sol_cost on chain -> TooMuchSolRequired -> 0% fill.
    #[test]
    fn confirm_flow_must_be_priced_or_the_request_exceeds_the_cap() {
        let p = params();
        let dev = 500_000_000u64; // 0.5 SOL dev buy
        let confirmers = 6_000_000_000u64; // 6 SOL confirmed ahead of us
        let prior_flow = dev + confirmers; // the real curve depth our buy lands in
        let budget = 2_800_000_000u64;

        // FIXED: price against the full prior flow -> the request fits under the cap.
        let good = p.plan_buy(prior_flow, budget, 30, 100);
        let good_cost = real_cost(&p, prior_flow, good.amount);
        assert!(
            good_cost <= good.max_sol_cost,
            "correct pricing must fit: cost {good_cost} cap {}",
            good.max_sol_cost
        );

        // BUG: pricing against the dev buy only, but executing at the real (deeper) curve,
        // over-requests tokens and exceeds the cap. This is what used to happen every fire.
        let bug = p.plan_buy(dev, budget, 30, 100);
        let bug_cost = real_cost(&p, prior_flow, bug.amount);
        assert!(
            bug_cost > bug.max_sol_cost,
            "dev-only pricing should over-request at the real depth: cost {bug_cost} cap {}",
            bug.max_sol_cost
        );
    }

    #[test]
    fn exact_sol_in_fixes_spend_and_floors_tokens() {
        let p = params();
        let prior = 6_000_000_000u64; // 6 SOL already in the curve ahead of us
        let budget = 2_000_000_000u64;
        let plan = p.plan_exact_sol_in(prior, budget, 640); // 6.4% slippage (his median)
        // sol_in is exactly the budget - the spend can never overshoot
        assert_eq!(plan.amount, budget);
        // min_tokens is the ~6.4% floor below the tokens expected at that depth
        let curve_sol = budget * 10_000 / (10_000 + p.total_fee_bps());
        let expected = p.tokens_behind_dev_buy(prior, curve_sol);
        assert!(plan.max_sol_cost < expected && plan.max_sol_cost > expected * 90 / 100);
        // at the expected depth we clear the floor and fill
        assert!(expected >= plan.max_sol_cost);
        // a big up-move (3 SOL more flow ahead) yields fewer tokens than the floor -> reverts,
        // which is the point: we don't fill late when we've been overtaken.
        let deeper = p.tokens_behind_dev_buy(prior + 3_000_000_000, curve_sol);
        assert!(deeper < plan.max_sol_cost, "a big up-move should fall below the floor");
    }

    /// Regression for the exact_sol_in cost-basis bug: under `buy_exact_sol_in` the wire
    /// field `max_sol_cost` carries a TOKEN floor (~1e13), not lamports. Ghost PnL, the
    /// seller's stop-loss basis and tip sizing all priced off it and booked garbage (the
    /// seller's value multiple read ~0, so the 0.8x stop dumped every position at slot +1).
    /// Both planners must therefore expose the budget and the expected fill explicitly,
    /// independent of which instruction the wire fields are shaped for.
    #[test]
    fn both_planners_carry_the_instruction_independent_cost_basis() {
        let p = params();
        let budget = 2_000_000_000u64;
        let prior = 6_000_000_000u64;

        let exact = p.plan_exact_sol_in(prior, budget, 640);
        assert_eq!(exact.budget_lamports, budget);
        assert_eq!(exact.amount, budget, "wire arg0 is sol_in");
        // the wire cap field is tokens here — orders of magnitude off lamports. Anyone
        // reading it as a cost basis is wrong; budget_lamports is the only cost basis.
        assert!(exact.max_sol_cost > 100 * budget, "min_tokens dwarfs any sane lamport cost");
        assert_ne!(exact.budget_lamports, exact.max_sol_cost);
        // expected fill sits above the slippage floor
        assert!(exact.expected_tokens > exact.max_sol_cost);

        let classic = p.plan_buy(prior, budget, 30, 100);
        assert_eq!(classic.budget_lamports, budget);
        assert_eq!(classic.expected_tokens, classic.amount);
        // classic cap is budget + slippage pad, never the budget itself
        assert!(classic.max_sol_cost > budget);
    }

    #[test]
    fn buy_plan_lands_close_to_the_budget_and_under_the_cap() {
        let p = params();
        let budget = 1_000_000_000u64; // 1 SOL
        for dev in [0u64, 500_000_000, 2_000_000_000] {
            let plan = p.plan_buy(dev, budget, 30, 100);
            let cost = real_cost(&p, dev, plan.amount);

            assert!(
                cost <= plan.max_sol_cost,
                "dev {dev}: cost {cost} must fit under cap {}",
                plan.max_sol_cost
            );
            // within 1% of the budget: that is the actual slippage we accept
            let diff = budget.abs_diff(cost);
            assert!(
                diff * 100 / budget <= 1,
                "dev {dev}: spent {cost}, budget {budget}, off by {diff}"
            );
        }
    }

    #[test]
    fn buy_plan_scales_with_the_budget() {
        let p = params();
        let small = p.plan_buy(0, 100_000_000, 30, 100);
        let big = p.plan_buy(0, 1_000_000_000, 30, 100);
        assert!(big.amount > small.amount);
        assert_eq!(big.max_sol_cost, 1_010_000_000);
        assert_eq!(small.max_sol_cost, 101_000_000);
    }

    fn hex_to_vec(s: &str) -> Vec<u8> {
        let s: String = s.chars().filter(|c| !c.is_whitespace()).collect();
        (0..s.len() / 2)
            .map(|i| u8::from_str_radix(&s[i * 2..i * 2 + 2], 16).unwrap())
            .collect()
    }

    /// create_v2 instruction data captured on mainnet at slot 441329124.
    const CREATE_V2_DATA: &str = "d6904cec5f8b31b408000000717569636b61646404000000736e61705000000068\
                                  747470733a2f2f697066732e696f2f697066732f6261666b726569643571726768\
                                  347a727a7477716f6134337763647a70346f37356d79646e7361656d7169343674\
                                  36643568657032377772746379cc0b9e3550884b962a59175e6a4757242d9ff32c\
                                  00b4044f1e2ed4afb65b30930100";

    #[test]
    fn reads_creator_argument_from_create_v2_data() {
        let data = hex_to_vec(CREATE_V2_DATA);
        assert_eq!(data.len(), 146);
        assert_eq!(&data[..8], &DISC_CREATE_V2);
        let creator = trailing_pubkey(&data, 2).unwrap();
        assert_eq!(
            creator.to_bytes(),
            [
                0xcc, 0x0b, 0x9e, 0x35, 0x50, 0x88, 0x4b, 0x96, 0x2a, 0x59, 0x17, 0x5e, 0x6a, 0x47,
                0x57, 0x24, 0x2d, 0x9f, 0xf3, 0x2c, 0x00, 0xb4, 0x04, 0x4f, 0x1e, 0x2e, 0xd4, 0xaf,
                0xb6, 0x5b, 0x30, 0x93
            ]
        );
    }

    #[test]
    fn constant_pdas_match_on_chain_addresses() {
        assert_eq!(
            Pubkey::find_program_address(&[b"global"], &PUMP_PROGRAM).0,
            GLOBAL
        );
        assert_eq!(
            Pubkey::find_program_address(&[b"__event_authority"], &PUMP_PROGRAM).0,
            EVENT_AUTHORITY
        );
        assert_eq!(
            Pubkey::find_program_address(&[b"global_volume_accumulator"], &PUMP_PROGRAM).0,
            GLOBAL_VOLUME_ACCUMULATOR
        );
    }

    /// `HXx1xm2r2yJNfa4VMeRLQZJ5f3Vbr93a6k1XisArxqag` is the remaining account a mainnet buy
    /// passed for mint `6Y9CWQBSqDKofUm2sRALQZ9Qvd721BVyFnL3duHDpump`.
    #[test]
    fn derives_bonding_curve_v2() {
        let mint = Pubkey::from_str_const("6Y9CWQBSqDKofUm2sRALQZ9Qvd721BVyFnL3duHDpump");
        assert_eq!(
            bonding_curve_v2(&mint),
            Pubkey::from_str_const("HXx1xm2r2yJNfa4VMeRLQZJ5f3Vbr93a6k1XisArxqag")
        );
    }

    #[test]
    fn buy_price_is_monotonic_and_capped() {
        let p = params();
        assert_eq!(p.initial_buy_price(0), 0);
        assert!(p.initial_buy_price(1_000_000_000) > p.initial_buy_price(500_000_000));
        assert!(p.initial_buy_price(u64::MAX / 2) <= p.initial_real_token_reserves);
        let alone = p.tokens_behind_dev_buy(0, 500_000_000);
        let behind = p.tokens_behind_dev_buy(2_000_000_000, 500_000_000);
        assert!(behind < alone);
    }
}
