//! Prints the byte-patch table for the buy template.
//!
//! The offsets are never hand-written: `template::build` compiles the instruction list once
//! with distinctive placeholder values and then locates each one in the serialized message.
//! Change the instruction list and the offsets regenerate themselves — this tool just shows
//! the result, so a layout change can be eyeballed without running the whole proxy.
//!
//! ```text
//! cargo run -p sniper --example dump_offsets            # default 90k CU
//! cargo run -p sniper --example dump_offsets -- 120000
//! ```

use solana_sdk::{message::VersionedMessage, pubkey::Pubkey};

fn main() {
    let cu_limit: u32 = std::env::args()
        .nth(1)
        .and_then(|a| a.parse().ok())
        .unwrap_or(90_000);

    // placeholder identities: only the layout matters here
    let accounts = sniper::template::StaticAccounts {
        fee_recipient: Pubkey::new_from_array([0xB1; 32]),
        buyback_fee_recipient: Pubkey::new_from_array([0xB2; 32]),
        user: Pubkey::new_from_array([0xB3; 32]),
        user_volume_accumulator: Pubkey::new_from_array([0xB4; 32]),
    };

    let template = sniper::template::build(&accounts, cu_limit, false);
    println!("compute unit limit: {cu_limit}");
    println!("{}", template.describe());

    let msg: VersionedMessage =
        bincode::deserialize(template.message()).expect("template is a valid v0 message");
    let VersionedMessage::V0(m) = msg else {
        panic!("expected a v0 message");
    };

    println!("\ninstructions:");
    for (i, ix) in m.instructions.iter().enumerate() {
        println!(
            "  {i}: program {} accounts {:?} data {} bytes",
            m.account_keys[ix.program_id_index as usize],
            ix.accounts,
            ix.data.len()
        );
    }
    println!("\naccount keys ({}):", m.account_keys.len());
    for (i, k) in m.account_keys.iter().enumerate() {
        println!("  {i}: {k}");
    }
}
