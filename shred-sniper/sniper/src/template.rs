//! Prebuilt buy transaction. Built once at startup, then byte-patched per launch. No
//! instruction building, no message compilation and no serialization on the hot path.
//!
//! The instruction layout mirrors a known-good mainnet sniper transaction:
//!
//! ```text
//! 0 System        advanceNonceAccount           durable nonce, no blockhash dependency
//! 1 System        transfer                      provider tip
//! 2 ComputeBudget setComputeUnitLimit
//! 3 ComputeBudget setComputeUnitPrice
//! 4 System        createAccountWithSeed         token account, 165 bytes
//! 5 Token(2022)   initializeAccount3
//! 6 pump.fun      buy                           18 accounts
//! ```
//!
//! Two things here are deliberate and non-obvious:
//!
//! * The token account is created with `createAccountWithSeed` + `initializeAccount3`
//!   instead of the associated-token-account program. The ATA program costs ~18k compute
//!   units and two extra CPI hops; this path costs ~2k. The reference transaction lands the
//!   whole buy in 68k CU, versus 89k for the ATA version.
//! * A durable nonce replaces the recent blockhash. The transaction never expires, so it
//!   does not depend on a fresh blockhash arriving in time, and — because every provider
//!   variant of one launch shares a nonce — exactly one of them can ever land.
//!
//! Layout of `tx`: `[1u8 signature count][64 byte signature][serialized v0 message]`.

use solana_sdk::{
    hash::Hash,
    instruction::{AccountMeta, Instruction},
    message::{v0, VersionedMessage},
    pubkey::Pubkey,
    system_program,
    sysvar::recent_blockhashes,
};

use crate::pumpfun::{
    EVENT_AUTHORITY, FEE_CONFIG, FEE_PROGRAM, GLOBAL, GLOBAL_VOLUME_ACCUMULATOR, PUMP_PROGRAM,
    TOKEN_2022_PROGRAM, TOKEN_PROGRAM,
};

/// Discriminator of pump.fun `buy`.
const DISC_BUY: [u8; 8] = [102, 6, 61, 18, 1, 218, 235, 234];

/// spl-token account size without extensions, and its rent-exempt minimum.
pub const TOKEN_ACCOUNT_SPACE: u64 = 165;
pub const TOKEN_ACCOUNT_RENT: u64 = 2_039_280;

/// Length of the ASCII seed used for the buy's token account.
pub const SEED_LEN: usize = 6;

pub const MSG_OFFSET: usize = 1 + 64;

/// Byte offsets (absolute, into `Template::tx`) of every field patched per launch.
#[derive(Debug, Clone, Copy)]
pub struct Offsets {
    // rotating pump.fun config
    pub fee_recipient: usize,
    pub buyback_fee_recipient: usize,
    // per launch
    pub mint: usize,
    pub bonding_curve: usize,
    pub associated_bonding_curve: usize,
    pub bonding_curve_v2: usize,
    pub creator_vault: usize,
    pub token_account: usize,
    /// the token program appears twice: as an account key, and as the `owner` field of
    /// createAccountWithSeed. Both have to be patched or the two disagree.
    pub token_program: [usize; 2],
    pub seed: usize,
    pub amount: usize,
    pub max_sol_cost: usize,
    // per nonce
    pub nonce_account: usize,
    pub nonce_value: usize,
    // per provider
    pub tip_account: usize,
    pub tip_lamports: usize,
    pub cu_price: usize,
}

#[derive(Debug, Clone)]
pub struct Template {
    pub tx: Vec<u8>,
    pub offsets: Offsets,
}

/// Accounts that stay the same for every launch, read from chain at startup.
#[derive(Debug, Clone, Copy)]
pub struct StaticAccounts {
    pub fee_recipient: Pubkey,
    pub buyback_fee_recipient: Pubkey,
    pub user: Pubkey,
    pub user_volume_accumulator: Pubkey,
}

// distinctive dummies so `find_unique` cannot match anything else in the message
const D_MINT: Pubkey = Pubkey::new_from_array([0xA1; 32]);
const D_CURVE: Pubkey = Pubkey::new_from_array([0xA2; 32]);
const D_ABC: Pubkey = Pubkey::new_from_array([0xA3; 32]);
const D_BCV2: Pubkey = Pubkey::new_from_array([0xA4; 32]);
const D_CREATOR_VAULT: Pubkey = Pubkey::new_from_array([0xA5; 32]);
const D_TOKEN_ACCOUNT: Pubkey = Pubkey::new_from_array([0xA6; 32]);
const D_TOKEN_PROGRAM: Pubkey = Pubkey::new_from_array([0xA7; 32]);
const D_TIP_ACCOUNT: Pubkey = Pubkey::new_from_array([0xA8; 32]);
const D_NONCE_VALUE: [u8; 32] = [0xA9; 32];
const D_FEE_RECIPIENT: Pubkey = Pubkey::new_from_array([0xAA; 32]);
const D_BUYBACK_RECIPIENT: Pubkey = Pubkey::new_from_array([0xAB; 32]);
const D_NONCE_ACCOUNT: Pubkey = Pubkey::new_from_array([0xAC; 32]);

const D_AMOUNT: u64 = 0x1111_1111_1111_1111;
const D_MAX_SOL: u64 = 0x2222_2222_2222_2222;
const D_TIP_LAMPORTS: u64 = 0x3333_3333_3333_3333;
const D_CU_PRICE: u64 = 0x4444_4444_4444_4444;
const D_SEED: &[u8; SEED_LEN] = b"\xB1\xB2\xB3\xB4\xB5\xB6";

fn ro(k: Pubkey) -> AccountMeta {
    AccountMeta::new_readonly(k, false)
}
fn rw(k: Pubkey) -> AccountMeta {
    AccountMeta::new(k, false)
}

/// Builds the buy transaction template.
///
/// The pump.fun `buy` account list is the 16 accounts from the on-chain IDL plus two
/// remaining accounts that the deployed program requires today: `bonding_curve_v2` and a
/// buyback fee recipient. Sending only the 16 IDL accounts fails with
/// `BuybackFeeRecipientMissing` (0x17ae) and, once the buyback recipient is added,
/// `InvalidBondingCurveV2` (0x17ba).
pub fn build(static_accounts: &StaticAccounts, cu_limit: u32) -> Template {
    let StaticAccounts {
        user,
        user_volume_accumulator,
        ..
    } = *static_accounts;

    let mut instructions = Vec::with_capacity(7);

    // 0. advance the durable nonce
    instructions.push(Instruction {
        program_id: system_program::id(),
        accounts: vec![
            rw(D_NONCE_ACCOUNT),
            ro(recent_blockhashes::id()),
            AccountMeta::new_readonly(user, true),
        ],
        data: 4u32.to_le_bytes().to_vec(),
    });

    // 1. provider tip
    let mut tip_data = Vec::with_capacity(12);
    tip_data.extend_from_slice(&2u32.to_le_bytes());
    tip_data.extend_from_slice(&D_TIP_LAMPORTS.to_le_bytes());
    instructions.push(Instruction {
        program_id: system_program::id(),
        accounts: vec![AccountMeta::new(user, true), rw(D_TIP_ACCOUNT)],
        data: tip_data,
    });

    // 2. compute unit limit
    let mut cu_limit_data = Vec::with_capacity(5);
    cu_limit_data.push(0x02u8);
    cu_limit_data.extend_from_slice(&cu_limit.to_le_bytes());
    instructions.push(Instruction {
        program_id: crate::pumpfun::COMPUTE_BUDGET_PROGRAM,
        accounts: vec![],
        data: cu_limit_data,
    });

    // 3. compute unit price
    let mut cu_price_data = Vec::with_capacity(9);
    cu_price_data.push(0x03u8);
    cu_price_data.extend_from_slice(&D_CU_PRICE.to_le_bytes());
    instructions.push(Instruction {
        program_id: crate::pumpfun::COMPUTE_BUDGET_PROGRAM,
        accounts: vec![],
        data: cu_price_data,
    });

    // 4. create the token account from a seed (cheaper than the ATA program)
    let mut create_data = Vec::with_capacity(4 + 32 + 8 + SEED_LEN + 8 + 8 + 32);
    create_data.extend_from_slice(&3u32.to_le_bytes());
    create_data.extend_from_slice(&user.to_bytes());
    create_data.extend_from_slice(&(SEED_LEN as u64).to_le_bytes());
    create_data.extend_from_slice(D_SEED);
    create_data.extend_from_slice(&TOKEN_ACCOUNT_RENT.to_le_bytes());
    create_data.extend_from_slice(&TOKEN_ACCOUNT_SPACE.to_le_bytes());
    create_data.extend_from_slice(&D_TOKEN_PROGRAM.to_bytes());
    instructions.push(Instruction {
        program_id: system_program::id(),
        accounts: vec![
            AccountMeta::new(user, true),
            rw(D_TOKEN_ACCOUNT),
            AccountMeta::new_readonly(user, true),
        ],
        data: create_data,
    });

    // 5. initializeAccount3(owner)
    let mut init_data = Vec::with_capacity(33);
    init_data.push(18u8);
    init_data.extend_from_slice(&user.to_bytes());
    instructions.push(Instruction {
        program_id: D_TOKEN_PROGRAM,
        accounts: vec![rw(D_TOKEN_ACCOUNT), ro(D_MINT)],
        data: init_data,
    });

    // 6. pump.fun buy
    let mut buy_data = Vec::with_capacity(25);
    buy_data.extend_from_slice(&DISC_BUY);
    buy_data.extend_from_slice(&D_AMOUNT.to_le_bytes());
    buy_data.extend_from_slice(&D_MAX_SOL.to_le_bytes());
    buy_data.push(0); // track_volume: OptionBool::None
    instructions.push(Instruction {
        program_id: PUMP_PROGRAM,
        accounts: vec![
            ro(GLOBAL),
            rw(D_FEE_RECIPIENT),
            ro(D_MINT),
            rw(D_CURVE),
            rw(D_ABC),
            rw(D_TOKEN_ACCOUNT),
            AccountMeta::new(user, true),
            ro(system_program::id()),
            ro(D_TOKEN_PROGRAM),
            rw(D_CREATOR_VAULT),
            ro(EVENT_AUTHORITY),
            ro(PUMP_PROGRAM),
            ro(GLOBAL_VOLUME_ACCUMULATOR),
            rw(user_volume_accumulator),
            ro(FEE_CONFIG),
            ro(FEE_PROGRAM),
            // remaining accounts, required by the deployed program
            rw(D_BCV2),
            rw(D_BUYBACK_RECIPIENT),
        ],
        data: buy_data,
    });

    let message =
        v0::Message::try_compile(&user, &instructions, &[], Hash::new_from_array(D_NONCE_VALUE))
            .expect("compile buy template");
    let msg_bytes = VersionedMessage::V0(message).serialize();

    let mut tx = Vec::with_capacity(MSG_OFFSET + msg_bytes.len());
    tx.push(1u8);
    tx.extend_from_slice(&[0u8; 64]);
    tx.extend_from_slice(&msg_bytes);

    let find = |needle: &[u8]| -> usize {
        MSG_OFFSET + find_unique(&msg_bytes, needle).expect("template placeholder must be unique")
    };
    let find_pair = |needle: &[u8]| -> [usize; 2] {
        let all = find_all(&msg_bytes, needle);
        assert_eq!(
            all.len(),
            2,
            "expected the token program placeholder exactly twice, found {}",
            all.len()
        );
        [MSG_OFFSET + all[0], MSG_OFFSET + all[1]]
    };

    let offsets = Offsets {
        fee_recipient: find(&D_FEE_RECIPIENT.to_bytes()),
        buyback_fee_recipient: find(&D_BUYBACK_RECIPIENT.to_bytes()),
        mint: find(&D_MINT.to_bytes()),
        bonding_curve: find(&D_CURVE.to_bytes()),
        associated_bonding_curve: find(&D_ABC.to_bytes()),
        bonding_curve_v2: find(&D_BCV2.to_bytes()),
        creator_vault: find(&D_CREATOR_VAULT.to_bytes()),
        token_account: find(&D_TOKEN_ACCOUNT.to_bytes()),
        token_program: find_pair(&D_TOKEN_PROGRAM.to_bytes()),
        seed: find(D_SEED),
        amount: find(&D_AMOUNT.to_le_bytes()),
        max_sol_cost: find(&D_MAX_SOL.to_le_bytes()),
        nonce_account: find(&D_NONCE_ACCOUNT.to_bytes()),
        nonce_value: find(&D_NONCE_VALUE),
        tip_account: find(&D_TIP_ACCOUNT.to_bytes()),
        tip_lamports: find(&D_TIP_LAMPORTS.to_le_bytes()),
        cu_price: find(&D_CU_PRICE.to_le_bytes()),
    };

    let mut t = Template { tx, offsets };
    // every current launch is `create_v2`, so token-2022 is the common case
    t.patch_token_program(&TOKEN_2022_PROGRAM.to_bytes());
    let off = t.offsets.fee_recipient;
    t.patch_key(off, &static_accounts.fee_recipient.to_bytes());
    let off = t.offsets.buyback_fee_recipient;
    t.patch_key(off, &static_accounts.buyback_fee_recipient.to_bytes());
    t
}

impl Template {
    #[inline(always)]
    pub fn patch_key(&mut self, offset: usize, key: &[u8; 32]) {
        self.tx[offset..offset + 32].copy_from_slice(key);
    }

    #[inline(always)]
    pub fn patch_u64(&mut self, offset: usize, value: u64) {
        self.tx[offset..offset + 8].copy_from_slice(&value.to_le_bytes());
    }

    /// Patches both copies of the token program id.
    #[inline(always)]
    pub fn patch_token_program(&mut self, key: &[u8; 32]) {
        let [a, b] = self.offsets.token_program;
        self.tx[a..a + 32].copy_from_slice(key);
        self.tx[b..b + 32].copy_from_slice(key);
    }

    #[inline(always)]
    pub fn patch_seed(&mut self, offset: usize, seed: &[u8; SEED_LEN]) {
        self.tx[offset..offset + SEED_LEN].copy_from_slice(seed);
    }

    pub fn message(&self) -> &[u8] {
        &self.tx[MSG_OFFSET..]
    }

    /// Human readable offset table, so the byte patch layout can be regenerated and
    /// eyeballed whenever the instruction list changes.
    pub fn describe(&self) -> String {
        let o = &self.offsets;
        format!(
            "tx {} bytes, message starts at {MSG_OFFSET}\n\
             fee_recipient            {:>5}\n\
             buyback_fee_recipient    {:>5}\n\
             mint                     {:>5}\n\
             bonding_curve            {:>5}\n\
             associated_bonding_curve {:>5}\n\
             bonding_curve_v2         {:>5}\n\
             creator_vault            {:>5}\n\
             token_account            {:>5}\n\
             token_program            {:>5} and {}\n\
             seed                     {:>5} ({SEED_LEN} bytes)\n\
             amount                   {:>5}\n\
             max_sol_cost             {:>5}\n\
             nonce_account            {:>5}\n\
             nonce_value              {:>5}\n\
             tip_account              {:>5}\n\
             tip_lamports             {:>5}\n\
             cu_price                 {:>5}",
            self.tx.len(),
            o.fee_recipient,
            o.buyback_fee_recipient,
            o.mint,
            o.bonding_curve,
            o.associated_bonding_curve,
            o.bonding_curve_v2,
            o.creator_vault,
            o.token_account,
            o.token_program[0],
            o.token_program[1],
            o.seed,
            o.amount,
            o.max_sol_cost,
            o.nonce_account,
            o.nonce_value,
            o.tip_account,
            o.tip_lamports,
            o.cu_price,
        )
    }
}


/// Every token account the sniper will ever create, worked out before the first launch.
///
/// The buy makes its token account with `createAccountWithSeed`, so the address is
/// `sha256(buyer || seed || token_program)`. The buyer is fixed at startup, the token program
/// is one of two known values, and the seed is a counter -- none of it depends on the launch.
/// So none of it belongs on the hot path.
///
/// Two things this buys beyond the ~90ns of sha256. `Pubkey::create_with_seed` is fallible,
/// and the hot path used to swallow that with `unwrap_or_default()`, which would have sent a
/// buy against `Pubkey::default()`; here a bad address is a startup error. And the seed for a
/// launch is now a pair of array indexes rather than a hash, so the whole step is a load.
pub struct SeedTable {
    seeds: Vec<[u8; SEED_LEN]>,
    /// address under token-2022, which is what every current launch uses
    token_2022: Vec<Pubkey>,
    /// address under classic spl-token, for a `create` that is not `create_v2`
    token_classic: Vec<Pubkey>,
}

impl SeedTable {
    /// Builds `len` entries starting from seed counter `start`.
    ///
    /// `start` is randomised per run so a restart cannot land on a token account an earlier
    /// run created and has not sold yet.
    pub fn build(buyer: &Pubkey, start: u32, len: usize) -> Result<Self, String> {
        let mut seeds = Vec::with_capacity(len);
        let mut token_2022 = Vec::with_capacity(len);
        let mut token_classic = Vec::with_capacity(len);
        for i in 0..len {
            let seed = seed_bytes(start.wrapping_add(i as u32) % 1_000_000);
            // safety: `seed_bytes` only ever produces ASCII digits
            let as_str = unsafe { core::str::from_utf8_unchecked(&seed) };
            let derive = |program: &Pubkey| {
                Pubkey::create_with_seed(buyer, as_str, program)
                    .map_err(|e| format!("seed {as_str} does not derive an address: {e}"))
            };
            token_2022.push(derive(&TOKEN_2022_PROGRAM)?);
            token_classic.push(derive(&TOKEN_PROGRAM)?);
            seeds.push(seed);
        }
        Ok(Self {
            seeds,
            token_2022,
            token_classic,
        })
    }

    pub fn len(&self) -> usize {
        self.seeds.len()
    }

    pub fn is_empty(&self) -> bool {
        self.seeds.is_empty()
    }

    /// Hot path. `index` must be less than `len()`.
    #[inline(always)]
    pub fn get(&self, index: usize, token_program: &Pubkey) -> ([u8; SEED_LEN], Pubkey) {
        let addresses = if *token_program == TOKEN_2022_PROGRAM {
            &self.token_2022
        } else {
            &self.token_classic
        };
        (self.seeds[index], addresses[index])
    }
}

/// Six ASCII digits, written straight into the template with no formatting machinery.
#[inline(always)]
pub fn seed_bytes(counter: u32) -> [u8; SEED_LEN] {
    let mut out = [b'0'; SEED_LEN];
    let mut v = counter % 1_000_000;
    let mut i = SEED_LEN;
    while i > 0 {
        i -= 1;
        out[i] = b'0' + (v % 10) as u8;
        v /= 10;
    }
    out
}

/// Every offset of `needle` in `haystack`.
fn find_all(haystack: &[u8], needle: &[u8]) -> Vec<usize> {
    let mut out = Vec::new();
    let mut i = 0;
    while i + needle.len() <= haystack.len() {
        if &haystack[i..i + needle.len()] == needle {
            out.push(i);
        }
        i += 1;
    }
    out
}

/// Returns the offset of `needle` in `haystack`, or None when it is missing or ambiguous.
fn find_unique(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    let mut found = None;
    let mut i = 0;
    while i + needle.len() <= haystack.len() {
        if &haystack[i..i + needle.len()] == needle {
            if found.is_some() {
                return None;
            }
            found = Some(i);
        }
        i += 1;
    }
    found
}

#[cfg(test)]
mod tests {
    use super::*;

    fn accounts() -> StaticAccounts {
        StaticAccounts {
            fee_recipient: Pubkey::new_from_array([0xB1; 32]),
            buyback_fee_recipient: Pubkey::new_from_array([0xB2; 32]),
            user: Pubkey::new_from_array([0xB3; 32]),
            user_volume_accumulator: Pubkey::new_from_array([0xB4; 32]),
        }
    }

    #[test]
    fn template_offsets_are_found_and_patchable() {
        let mut t = build(&accounts(), 90_000);
        assert_eq!(t.tx[0], 1);

        let mint = [0x5A; 32];
        let off = t.offsets.mint;
        t.patch_key(off, &mint);
        assert_eq!(&t.tx[off..off + 32], &mint);

        let off = t.offsets.amount;
        t.patch_u64(off, 12345);
        assert_eq!(
            u64::from_le_bytes(t.tx[off..off + 8].try_into().unwrap()),
            12345
        );

        let off = t.offsets.seed;
        t.patch_seed(off, b"008113");
        assert_eq!(&t.tx[off..off + SEED_LEN], b"008113");

        // both copies of the token program must move together
        let classic = crate::pumpfun::TOKEN_PROGRAM.to_bytes();
        t.patch_token_program(&classic);
        let [a, b] = t.offsets.token_program;
        assert_eq!(&t.tx[a..a + 32], &classic);
        assert_eq!(&t.tx[b..b + 32], &classic);
    }

    /// The instruction list must match the reference mainnet transaction:
    /// advanceNonce, tip, cu limit, cu price, createAccountWithSeed, initializeAccount3, buy.
    #[test]
    fn template_matches_the_reference_instruction_layout() {
        let t = build(&accounts(), 90_000);
        let msg: VersionedMessage = bincode::deserialize(t.message()).expect("valid v0 message");
        let VersionedMessage::V0(m) = msg else {
            panic!("expected v0");
        };
        assert_eq!(m.instructions.len(), 7);

        let system = m
            .account_keys
            .iter()
            .position(|k| *k == system_program::id())
            .unwrap() as u8;
        // 0: advance nonce
        assert_eq!(m.instructions[0].program_id_index, system);
        assert_eq!(m.instructions[0].data, 4u32.to_le_bytes());
        assert_eq!(m.instructions[0].accounts.len(), 3);
        // 1: tip transfer
        assert_eq!(m.instructions[1].program_id_index, system);
        assert_eq!(m.instructions[1].data[..4], 2u32.to_le_bytes());
        // 2,3: compute budget
        assert_eq!(m.instructions[2].data[0], 0x02);
        assert_eq!(m.instructions[3].data[0], 0x03);
        // 4: create account with seed, 165 bytes, rent exempt
        assert_eq!(m.instructions[4].data[..4], 3u32.to_le_bytes());
        // 4 disc | 32 base | 8 seed len | SEED_LEN seed | 8 lamports | 8 space | 32 owner
        let d = &m.instructions[4].data;
        assert_eq!(d.len(), 4 + 32 + 8 + SEED_LEN + 8 + 8 + 32);
        assert_eq!(u64::from_le_bytes(d[36..44].try_into().unwrap()), SEED_LEN as u64);
        let lamports_at = 44 + SEED_LEN;
        let lamports = u64::from_le_bytes(d[lamports_at..lamports_at + 8].try_into().unwrap());
        let space = u64::from_le_bytes(d[lamports_at + 8..lamports_at + 16].try_into().unwrap());
        assert_eq!(lamports, TOKEN_ACCOUNT_RENT);
        assert_eq!(space, TOKEN_ACCOUNT_SPACE);
        // 5: initializeAccount3
        assert_eq!(m.instructions[5].data[0], 18);
        assert_eq!(m.instructions[5].data.len(), 33);
        // 6: buy, 18 accounts, 25 bytes ending in the OptionBool byte
        let buy = &m.instructions[6];
        assert_eq!(buy.accounts.len(), 18);
        assert_eq!(buy.data.len(), 25);
        assert_eq!(buy.data[..8], DISC_BUY);
        assert_eq!(buy.data[24], 0);
    }

    /// The table is the hot path's only source of token accounts, so it has to agree with
    /// the derivation it replaced, for both token programs and at the wrap-around.
    #[test]
    fn precomputed_token_accounts_match_create_with_seed() {
        let buyer = Pubkey::new_from_array([0xB3; 32]);
        let table = SeedTable::build(&buyer, 999_990, 32).expect("table builds");
        assert_eq!(table.len(), 32);

        for i in 0..table.len() {
            for program in [TOKEN_2022_PROGRAM, TOKEN_PROGRAM] {
                let (seed, address) = table.get(i, &program);
                let as_str = std::str::from_utf8(&seed).unwrap();
                let expected = Pubkey::create_with_seed(&buyer, as_str, &program).unwrap();
                assert_eq!(address, expected, "entry {i} under {program}");
            }
        }

        // the counter wraps at a million, exactly as it did before
        assert_eq!(&table.get(0, &TOKEN_2022_PROGRAM).0, b"999990");
        assert_eq!(&table.get(9, &TOKEN_2022_PROGRAM).0, b"999999");
        assert_eq!(&table.get(10, &TOKEN_2022_PROGRAM).0, b"000000");

        // anything that is not token-2022 is treated as classic spl-token, which is what the
        // template patches in that case
        let (_, classic) = table.get(3, &TOKEN_PROGRAM);
        let (_, unknown) = table.get(3, &Pubkey::new_from_array([0x77; 32]));
        assert_eq!(classic, unknown);
        assert_ne!(classic, table.get(3, &TOKEN_2022_PROGRAM).1);
    }

    #[test]
    fn seeds_are_six_ascii_digits() {
        assert_eq!(&seed_bytes(8113), b"008113");
        assert_eq!(&seed_bytes(0), b"000000");
        assert_eq!(&seed_bytes(999_999), b"999999");
        assert_eq!(&seed_bytes(1_000_000), b"000000");
    }

    /// The token account address must be reproducible from base + seed + token program,
    /// because the sell side has to find it again.
    #[test]
    fn token_account_is_derivable_from_the_seed() {
        let user = Pubkey::new_from_array([0xB3; 32]);
        let seed = seed_bytes(8113);
        let seed = std::str::from_utf8(&seed).unwrap();
        let a = Pubkey::create_with_seed(&user, seed, &TOKEN_2022_PROGRAM).unwrap();
        let b = Pubkey::create_with_seed(&user, seed, &TOKEN_2022_PROGRAM).unwrap();
        assert_eq!(a, b);
        let other = Pubkey::create_with_seed(&user, "008114", &TOKEN_2022_PROGRAM).unwrap();
        assert_ne!(a, other);
    }
}
