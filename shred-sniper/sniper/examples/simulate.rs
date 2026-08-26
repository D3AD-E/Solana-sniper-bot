//! End-to-end check of the buy path against mainnet, without sending anything.
//!
//! Builds the same template the hot path uses, patches it with a real mint's accounts and a
//! real durable nonce, signs it and runs `simulateTransaction`. A clean simulation means the
//! account list, instruction data, nonce and compute budget all match the deployed program.
//!
//! ```text
//! cargo run -p sniper --example simulate -- sniper.json <mint currently on the curve>
//! ```
//!
//! Requires the config's `nonce_accounts` to exist and be owned by the buying wallet:
//!
//! ```text
//! solana-keygen new -o nonce1.json
//! solana create-nonce-account nonce1.json 0.0015 --nonce-authority <buyer>
//! ```

use std::{path::Path, str::FromStr};

use solana_client::{rpc_client::RpcClient, rpc_config::RpcSimulateTransactionConfig};
use solana_sdk::{
    commitment_config::CommitmentConfig, pubkey::Pubkey, transaction::VersionedTransaction,
};

use sniper::{chain, config::SniperConfig, pumpfun, template};

fn main() {
    let mut args = std::env::args().skip(1);
    let config_path = args.next().unwrap_or_else(|| "sniper.json".to_string());
    let mint_arg = args
        .next()
        .expect("usage: simulate <config.json> <mint pubkey>");
    let mint = Pubkey::from_str(&mint_arg).expect("mint must be a base58 pubkey");

    let cfg = SniperConfig::load(Path::new(&config_path)).expect("config");
    let signing_key = read_keypair(&cfg.keypair_path);
    let buyer = Pubkey::new_from_array(signing_key.verifying_key().to_bytes());
    println!("buyer: {buyer}");

    let client = RpcClient::new_with_commitment(cfg.rpc_url.clone(), CommitmentConfig::confirmed());
    let global = chain::fetch_global(&cfg.rpc_url).expect("global account");

    // nonce, exactly as the hot path takes it
    let nonce_keys = cfg
        .nonce_accounts
        .iter()
        .map(|s| s.parse::<Pubkey>().expect("nonce account pubkey"))
        .collect::<Vec<_>>();
    assert!(
        !nonce_keys.is_empty(),
        "config needs at least one nonce_account"
    );
    let nonces = chain::NoncePool::new(nonce_keys);
    nonces
        .load_once(&cfg.rpc_url, &buyer)
        .expect("nonce accounts must exist and be owned by the buyer");
    let (nonce_account, nonce_value) = nonces.take().expect("a nonce value");
    println!("nonce account: {}", Pubkey::new_from_array(nonce_account));

    // resolve the launch the way the hot path does
    let bonding_curve = pumpfun::bonding_curve(&mint);
    let curve_data = client
        .get_account_data(&bonding_curve)
        .expect("bonding curve account (is the token still on the curve?)");
    let creator = Pubkey::new_from_array(curve_data[49..81].try_into().unwrap());
    let virtual_token_reserves = u64::from_le_bytes(curve_data[8..16].try_into().unwrap());
    let virtual_sol_reserves = u64::from_le_bytes(curve_data[16..24].try_into().unwrap());

    let token_program = client.get_account(&mint).expect("mint account").owner;
    let associated_bonding_curve =
        pumpfun::associated_token_address(&bonding_curve, &token_program, &mint);
    let creator_vault = pumpfun::creator_vault(&creator);
    let bonding_curve_v2 = pumpfun::bonding_curve_v2(&mint);

    // a seed that is very unlikely to already exist
    let seed = template::seed_bytes(rand_seed());
    let seed_str = std::str::from_utf8(&seed).unwrap();
    let token_account =
        Pubkey::create_with_seed(&buyer, seed_str, &token_program).expect("seeded token account");

    println!("token program:  {token_program}");
    println!("bonding curve:  {bonding_curve}");
    println!("curve ATA:      {associated_bonding_curve}");
    println!("creator:        {creator}");
    println!("creator vault:  {creator_vault}");
    println!("curve v2:       {bonding_curve_v2}");
    println!("token account:  {token_account} (seed {seed_str})");

    let static_accounts = template::StaticAccounts {
        fee_recipient: global.fee_recipient,
        buyback_fee_recipient: global.buyback_fee_recipients[0],
        user: buyer,
        user_volume_accumulator: pumpfun::user_volume_accumulator(&buyer),
    };

    // price off the live curve rather than the initial reserves
    let live = pumpfun::CurveParams {
        initial_virtual_sol_reserves: virtual_sol_reserves,
        initial_virtual_token_reserves: virtual_token_reserves,
        initial_real_token_reserves: u64::MAX,
        fee_basis_points: global.fee_basis_points,
        creator_fee_basis_points: global.creator_fee_basis_points,
    };
    // simulate with a small budget so the wallet balance is never the reason it fails
    let budget = 2_000_000u64;
    let plan = live.plan_buy(0, budget, cfg.haircut_bps, cfg.slippage_bps);
    println!(
        "plan: {} tokens, cap {} lamports (budget {budget})",
        plan.amount, plan.max_sol_cost
    );

    let provider = cfg
        .providers
        .iter()
        .find(|p| p.enabled)
        .or_else(|| cfg.providers.first())
        .expect("at least one provider for tip settings");
    let tip_account = provider.tip_account_bytes().expect("tip accounts")[0];

    let mut tmpl = template::build(&static_accounts, cfg.cu_limit, false);
    let o = tmpl.offsets;
    tmpl.patch_key(o.mint, &mint.to_bytes());
    tmpl.patch_key(o.bonding_curve, &bonding_curve.to_bytes());
    tmpl.patch_key(
        o.associated_bonding_curve,
        &associated_bonding_curve.to_bytes(),
    );
    tmpl.patch_key(o.bonding_curve_v2, &bonding_curve_v2.to_bytes());
    tmpl.patch_key(o.creator_vault, &creator_vault.to_bytes());
    tmpl.patch_key(o.token_account, &token_account.to_bytes());
    tmpl.patch_token_program(&token_program.to_bytes());
    tmpl.patch_seed(o.seed, &seed);
    tmpl.patch_u64(o.amount, plan.amount);
    tmpl.patch_u64(o.max_sol_cost, plan.max_sol_cost);
    tmpl.patch_key(o.nonce_account, &nonce_account);
    tmpl.patch_key(o.nonce_value, &nonce_value);
    tmpl.patch_key(o.tip_account, &tip_account);
    tmpl.patch_u64(o.tip_lamports, provider.tip_lamports);
    tmpl.patch_u64(o.cu_price, provider.cu_price);

    let signature = {
        use ed25519_dalek::Signer;
        signing_key.sign(&tmpl.tx[template::MSG_OFFSET..])
    };
    tmpl.tx[1..65].copy_from_slice(&signature.to_bytes());

    let tx: VersionedTransaction =
        bincode::deserialize(&tmpl.tx).expect("patched bytes are a valid transaction");
    let result = client
        .simulate_transaction_with_config(
            &tx,
            RpcSimulateTransactionConfig {
                sig_verify: true,
                replace_recent_blockhash: false,
                commitment: Some(CommitmentConfig::confirmed()),
                ..Default::default()
            },
        )
        .expect("simulate");

    println!(
        "\n=== {} bytes, err {:?}, units {:?}",
        tmpl.tx.len(),
        result.value.err,
        result.value.units_consumed
    );
    for line in result.value.logs.unwrap_or_default() {
        println!("   {line}");
    }
}

fn rand_seed() -> u32 {
    use std::time::{SystemTime, UNIX_EPOCH};
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.subsec_nanos() % 1_000_000)
        .unwrap_or(1)
}

fn read_keypair(path: &str) -> ed25519_dalek::SigningKey {
    let raw = std::fs::read_to_string(path).expect("keypair file");
    let bytes: Vec<u8> = serde_json::from_str(&raw).expect("keypair json array");
    let seed: [u8; 32] = bytes[..32].try_into().expect("keypair is 64 bytes");
    ed25519_dalek::SigningKey::from_bytes(&seed)
}
