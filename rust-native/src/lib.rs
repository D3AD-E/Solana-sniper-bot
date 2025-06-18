use base64::{Engine as _, engine::general_purpose};
use ed25519_dalek::{Signer, SigningKey};
use napi::bindgen_prelude::*;
use napi_derive::napi;
use num_bigint::BigUint;
use solana_sdk::{
    instruction::CompiledInstruction,
    message::{VersionedMessage, v0::Message as V0Message},
    pubkey::Pubkey,
    transaction::VersionedTransaction,
};
use spl_associated_token_account::get_associated_token_address;

/// Object returned to JS
#[napi(object)]
pub struct ParsedTx {
    pub signer: String,
    pub mint: String,
    pub curve: String,
    pub owner_vault: String,
    pub other_value: String,
}

#[napi]
pub fn sign_message(message: Buffer, secret_key: Buffer) -> Result<String> {
    // secret_key = 64 bytes: [32-byte seed | 32-byte pubkey]
    if secret_key.len() < 32 {
        return Err(Error::from_reason("secret key must be ≥32 bytes"));
    }

    // --- 1. sign the message --------------------------------
    let seed: [u8; 32] = secret_key[0..32]
        .try_into()
        .map_err(|_| Error::from_reason("failed to slice seed"))?;
    let sig = SigningKey::from_bytes(&seed).sign(&message); // 64-byte Ed25519

    // --- 2. build raw-tx  header(1) + sig(64) + msg ---------
    let mut raw = Vec::with_capacity(1 + 64 + message.len());
    raw.push(1u8); // signature-count = 1  (LEB128)
    raw.extend_from_slice(&sig.to_bytes());
    raw.extend_from_slice(&message);

    // --- 3. base-64 output ----------------------------------
    Ok(general_purpose::STANDARD.encode(raw))
}

/// JS export: `associatedTokenAddress(...)`
///   mint_buf  – 32-byte Buffer     (mint public key)
///   owner_buf – 32-byte Buffer     (wallet pubkey)
/// Returns     – 32-byte Buffer     (ATA pubkey)
#[napi(js_name = "associatedTokenAddress")]
pub fn associated_token_address(mint_buf: Buffer, owner_buf: Buffer) -> Result<Buffer> {
    if mint_buf.len() != 32 || owner_buf.len() != 32 {
        return Err(Error::from_reason(
            "mint and owner must be 32-byte public keys",
        ));
    }

    let mint_array: [u8; 32] = mint_buf[..]
        .try_into()
        .map_err(|_| Error::from_reason("mint not 32 bytes"))?;
    let owner_array: [u8; 32] = owner_buf[..]
        .try_into()
        .map_err(|_| Error::from_reason("owner not 32 bytes"))?;
    let mint = Pubkey::new_from_array(mint_array);
    let owner = Pubkey::new_from_array(owner_array);

    let ata = get_associated_token_address(&owner, &mint);
    Ok(Buffer::from(ata.to_bytes().to_vec()))
}

/// JS export: `findProgramAddress(...)`
///   seed_buf     – arbitrary seed bytes (e.g. your `staticSeed`)
///   mint_buf     – 32-byte Buffer
///   program_id   – base58 string
/// Returns        – 32-byte Buffer  (PDA pubkey)
#[napi(js_name = "findProgramAddress")]
pub fn find_program_address(
    seed_buf: Buffer,
    mint_buf: Buffer,
    program_id: String,
) -> Result<Buffer> {
    if mint_buf.len() != 32 {
        return Err(Error::from_reason("mint must be 32-byte public key"));
    }

    let program_key = Pubkey::from_str_const(&program_id);
    let mint_array: [u8; 32] = mint_buf[..]
        .try_into()
        .map_err(|_| Error::from_reason("mint not 32 bytes"))?;
    let mint = Pubkey::new_from_array(mint_array);

    let seeds: &[&[u8]] = &[&seed_buf, &mint.to_bytes()];
    let (pda, _bump) = Pubkey::find_program_address(seeds, &program_key);

    Ok(Buffer::from(pda.to_bytes().to_vec()))
}

/// entry_b64         – the tx as base-64
/// init_sol          – initialVirtualSolReserves  (decimal string)
/// init_token        – initialVirtualTokenReserves(decimal string)
#[napi]
pub fn parse_buy_tx(entry_b64: String, init_sol: String, init_token: String) -> Result<ParsedTx> {
    /* 1 ── base-64 → VersionedTransaction ──────────────────── */
    let raw = general_purpose::STANDARD
        .decode(entry_b64)
        .map_err(|e| Error::from_reason(format!("base64: {e}")))?;

    let tx: VersionedTransaction =
        bincode::deserialize(&raw).map_err(|e| Error::from_reason(format!("bincode: {e}")))?;

    /* 2 ── obtain (account_keys, instructions) for V0 *or* legacy */
    let (account_keys, instructions): (&[Pubkey], &[CompiledInstruction]) = match &tx.message {
        VersionedMessage::V0(v0) => (&v0.account_keys, &v0.instructions),
        VersionedMessage::Legacy(lo) => (&lo.account_keys, &lo.instructions),
    };

    /* 3 ── choose instructions whose accounts.len() > 5 ─────── */
    let long_ix: Vec<&CompiledInstruction> = instructions
        .iter()
        .filter(|ix| ix.accounts.len() > 5)
        .collect();
    if long_ix.is_empty() {
        return Err(Error::from_reason("no instruction >5 accounts"));
    }
    let first = long_ix[0];
    let last = *long_ix.last().unwrap();

    /* 4 ── same index logic as TypeScript (accounts only) ───── */
    if first.accounts.len() < 4 || last.accounts.len() < 10 {
        return Err(Error::from_reason("instruction accounts vec too short"));
    }
    let mint_idx = first.accounts[0] as usize;
    let curve_idx = first.accounts[3] as usize;
    let owner_vault_idx = last.accounts[9] as usize;

    /* 5 ── buyValue = u64-LE in data[8..16] ─────────────────── */
    if last.data.len() < 16 {
        return Err(Error::from_reason("instr data <16 bytes"));
    }
    let mut amt_le = [0u8; 8];
    amt_le.copy_from_slice(&last.data[8..16]);
    let buy_value = BigUint::from(u64::from_le_bytes(amt_le));

    /* 6 ── other = x₀·buy / (y₀−buy) (big-int) ─────────────── */
    let x0 = BigUint::parse_bytes(init_sol.as_bytes(), 10)
        .ok_or_else(|| Error::from_reason("init_sol not decimal"))?;
    let y0 = BigUint::parse_bytes(init_token.as_bytes(), 10)
        .ok_or_else(|| Error::from_reason("init_token not decimal"))?;
    if y0 <= buy_value {
        return Err(Error::from_reason("buy ≥ initial token reserve"));
    }
    let other = (&x0 * &buy_value) / (&y0 - &buy_value);

    /* 7 ── produce JS-friendly struct ───────────────────────── */
    Ok(ParsedTx {
        signer: account_keys[0].to_string(),
        mint: account_keys[mint_idx].to_string(),
        curve: account_keys[curve_idx].to_string(),
        owner_vault: account_keys[owner_vault_idx].to_string(),
        other_value: other.to_str_radix(10),
    })
}
