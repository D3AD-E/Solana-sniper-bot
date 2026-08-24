//! Detection is checked against real mainnet `create_v2` transactions, captured with
//! `getTransaction`. These are exactly the launches the previous filter dropped: they use
//! token-2022 and carry no metaplex metadata account at all.

use base64::{engine::general_purpose::STANDARD, Engine};
use solana_sdk::{pubkey::Pubkey, transaction::VersionedTransaction};

const FIXTURES: &str = include_str!("mainnet_creates.json");

fn transactions() -> Vec<VersionedTransaction> {
    let value: serde_json::Value = serde_json::from_str(FIXTURES).expect("fixture json");
    value
        .as_object()
        .expect("fixture is an object")
        .values()
        .map(|v| {
            let bytes = STANDARD.decode(v.as_str().expect("base64 string")).unwrap();
            bincode::deserialize::<VersionedTransaction>(&bytes).expect("transaction")
        })
        .collect()
}

#[test]
fn detects_every_captured_create_v2() {
    let txs = transactions();
    assert_eq!(txs.len(), 2, "fixture should hold two launches");
    for tx in &txs {
        let info = sniper::pumpfun::parse_create(tx).expect("create_v2 must be detected");
        assert!(info.is_v2);
        assert_eq!(info.token_program, sniper::pumpfun::TOKEN_2022_PROGRAM);
        assert_ne!(info.mint, Pubkey::default());
        assert_ne!(info.creator, Pubkey::default());
        // the bonding curve account in the instruction must match the PDA for the mint
        assert_eq!(info.bonding_curve, sniper::pumpfun::bonding_curve(&info.mint));
        // these launches bundle a dev buy in the same transaction
        assert!(info.dev_buy_lamports > 0, "dev buy should be picked up");
    }
}

/// The filter that shipped before required all four of: pump program, metaplex metadata,
/// classic spl-token and the system program. Every one of these launches fails that test,
/// which is why they were never emitted.
#[test]
fn old_four_account_filter_would_have_dropped_these() {
    const METAPLEX: Pubkey = Pubkey::from_str_const("metaqbxxUerdq28cj1RbAWkYQm3ybzjb6a8bt518x1s");
    const CLASSIC_TOKEN: Pubkey =
        Pubkey::from_str_const("TokenkegQfeZyiNwAJbNbGKPFXCWuBvf9Ss623VQ5DA");

    for tx in &transactions() {
        let keys = tx.message.static_account_keys();
        assert!(
            !keys.contains(&METAPLEX) || !keys.contains(&CLASSIC_TOKEN),
            "fixture should not satisfy the old required-account set"
        );
        assert!(
            sniper::pumpfun::parse_create(tx).is_some(),
            "but the discriminator based check must still find it"
        );
    }
}

#[test]
fn ignores_transactions_that_are_not_pump_creates() {
    // a transaction with no pump instruction at all
    let tx = VersionedTransaction::default();
    assert!(sniper::pumpfun::parse_create(&tx).is_none());
}
