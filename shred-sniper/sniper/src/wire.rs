//! Walking the serialized entry payload without deserializing it.
//!
//! A segment is a bincode `Vec<Entry>`, and decoding one costs ~43µs because every
//! transaction allocates vectors for its signatures, account keys and instruction data. When
//! all we want is the one transaction that contains a pump.fun create, almost all of that is
//! wasted.
//!
//! The layout is walkable: bincode writes `Vec` lengths as little-endian `u64`, while
//! `VersionedTransaction` and the message inside it use Solana's `short_vec` (compact-u16)
//! for their own arrays. So the byte span of each transaction can be computed by reading
//! lengths and skipping, and only the transaction whose span contains a create discriminator
//! is handed to bincode.
//!
//! Every function here returns `None` rather than guessing when the bytes do not walk
//! cleanly, and the caller falls back to a full deserialize. Being slow is acceptable; being
//! wrong is not.

use solana_sdk::transaction::VersionedTransaction;

/// `short_vec` / compact-u16: 7 bits per byte, high bit continues.
#[inline]
fn read_compact_u16(buf: &[u8], pos: &mut usize) -> Option<u16> {
    let mut value: u32 = 0;
    for i in 0..3 {
        let byte = *buf.get(*pos)?;
        *pos += 1;
        value |= ((byte & 0x7f) as u32) << (i * 7);
        if byte & 0x80 == 0 {
            // canonical encoding: no redundant continuation bytes
            if i > 0 && byte == 0 {
                return None;
            }
            return u16::try_from(value).ok();
        }
    }
    None
}

#[inline]
fn read_u64(buf: &[u8], pos: &mut usize) -> Option<u64> {
    let bytes: [u8; 8] = buf.get(*pos..*pos + 8)?.try_into().ok()?;
    *pos += 8;
    Some(u64::from_le_bytes(bytes))
}

#[inline]
fn skip(buf: &[u8], pos: &mut usize, n: usize) -> Option<()> {
    let next = pos.checked_add(n)?;
    if next > buf.len() {
        return None;
    }
    *pos = next;
    Some(())
}

/// Returns the offset just past the transaction starting at `start`.
///
/// Layout: `short_vec<Signature>` then the message, which is either legacy or, when the
/// first byte has the high bit set, versioned. A v0 message carries address table lookups
/// after its instructions.
pub fn transaction_end(buf: &[u8], start: usize) -> Option<usize> {
    let mut p = start;

    let signatures = read_compact_u16(buf, &mut p)?;
    skip(buf, &mut p, signatures as usize * 64)?;

    let first = *buf.get(p)?;
    let versioned = first & 0x80 != 0;
    if versioned {
        if first & 0x7f != 0 {
            return None; // only v0 exists today
        }
        p += 1;
    }

    skip(buf, &mut p, 3)?; // message header
    let keys = read_compact_u16(buf, &mut p)?;
    skip(buf, &mut p, keys as usize * 32)?;
    skip(buf, &mut p, 32)?; // recent blockhash

    let instructions = read_compact_u16(buf, &mut p)?;
    for _ in 0..instructions {
        skip(buf, &mut p, 1)?; // program id index
        let accounts = read_compact_u16(buf, &mut p)?;
        skip(buf, &mut p, accounts as usize)?;
        let data = read_compact_u16(buf, &mut p)?;
        skip(buf, &mut p, data as usize)?;
    }

    if versioned {
        let lookups = read_compact_u16(buf, &mut p)?;
        for _ in 0..lookups {
            skip(buf, &mut p, 32)?; // table account
            let writable = read_compact_u16(buf, &mut p)?;
            skip(buf, &mut p, writable as usize)?;
            let readonly = read_compact_u16(buf, &mut p)?;
            skip(buf, &mut p, readonly as usize)?;
        }
    }

    Some(p)
}

/// Deserializes only the transactions whose byte span contains one of `hits`.
///
/// `hits` are offsets of create discriminators, already located with a substring search.
/// Walking stops once past the last hit, so the tail of the segment is never touched.
pub fn transactions_at(payload: &[u8], hits: &[usize]) -> Option<Vec<VersionedTransaction>> {
    let last_hit = *hits.last()?;
    let mut p = 0usize;
    let entries = read_u64(payload, &mut p)?;
    let mut found = Vec::new();

    for _ in 0..entries {
        if p > last_hit {
            break;
        }
        read_u64(payload, &mut p)?; // num_hashes
        skip(payload, &mut p, 32)?; // hash
        let transactions = read_u64(payload, &mut p)?;

        for _ in 0..transactions {
            let start = p;
            let end = transaction_end(payload, start)?;
            if hits.iter().any(|h| *h >= start && *h < end) {
                let tx: VersionedTransaction =
                    bincode::deserialize(payload.get(start..end)?).ok()?;
                found.push(tx);
            }
            p = end;
            if p > last_hit {
                return Some(found);
            }
        }
    }
    Some(found)
}

#[cfg(test)]
mod tests {
    use super::*;
    use solana_sdk::{
        hash::Hash,
        instruction::{AccountMeta, Instruction},
        message::{v0, Message, VersionedMessage},
        pubkey::Pubkey,
        signature::Signature,
    };

    fn legacy_tx(data: Vec<u8>) -> VersionedTransaction {
        let ix = Instruction {
            program_id: Pubkey::new_unique(),
            accounts: vec![AccountMeta::new(Pubkey::new_unique(), true)],
            data,
        };
        VersionedTransaction {
            signatures: vec![Signature::default()],
            message: VersionedMessage::Legacy(Message::new(&[ix], Some(&Pubkey::new_unique()))),
        }
    }

    fn v0_tx(data: Vec<u8>) -> VersionedTransaction {
        let payer = Pubkey::new_unique();
        let ix = Instruction {
            program_id: Pubkey::new_unique(),
            accounts: vec![AccountMeta::new(Pubkey::new_unique(), false)],
            data,
        };
        let msg = v0::Message::try_compile(&payer, &[ix], &[], Hash::default()).unwrap();
        VersionedTransaction {
            signatures: vec![Signature::default()],
            message: VersionedMessage::V0(msg),
        }
    }

    #[test]
    fn compact_u16_matches_short_vec_encoding() {
        for value in [0u16, 1, 127, 128, 255, 16383, 16384, 65535] {
            let encoded = bincode::serialize(&solana_sdk::short_vec::ShortU16(value)).unwrap();
            let mut pos = 0;
            assert_eq!(read_compact_u16(&encoded, &mut pos), Some(value), "{value}");
            assert_eq!(pos, encoded.len(), "consumed wrong byte count for {value}");
        }
    }

    #[test]
    fn transaction_end_matches_serialized_length() {
        for tx in [legacy_tx(vec![1, 2, 3]), v0_tx(vec![9; 200]), v0_tx(vec![])] {
            let bytes = bincode::serialize(&tx).unwrap();
            assert_eq!(
                transaction_end(&bytes, 0),
                Some(bytes.len()),
                "walked length disagreed with serialized length"
            );
        }
    }

    #[test]
    fn walks_a_multi_transaction_entry_vector() {
        let marker = crate::pumpfun::DISC_CREATE_V2;
        let mut data = marker.to_vec();
        data.extend_from_slice(&[0xEE; 40]);

        let entries = vec![
            solana_entry::entry::Entry {
                num_hashes: 3,
                hash: Hash::default(),
                transactions: vec![legacy_tx(vec![7; 10]), v0_tx(vec![8; 30])],
            },
            solana_entry::entry::Entry {
                num_hashes: 1,
                hash: Hash::default(),
                transactions: vec![v0_tx(data.clone()), legacy_tx(vec![5; 5])],
            },
        ];
        let payload = bincode::serialize(&entries).unwrap();
        let hits: Vec<usize> = memchr::memmem::find_iter(&payload, &marker).collect();
        assert_eq!(hits.len(), 1, "marker should appear once");

        let found = transactions_at(&payload, &hits).expect("payload must walk");
        assert_eq!(found.len(), 1, "only the marked transaction is decoded");
        let expected = &entries[1].transactions[0];
        assert_eq!(found[0].signatures, expected.signatures);
        assert_eq!(found[0].message, expected.message);
    }

    #[test]
    fn refuses_truncated_or_nonsense_payloads() {
        assert_eq!(transaction_end(&[], 0), None);
        assert_eq!(transaction_end(&[0xff, 0xff, 0xff], 0), None);
        assert_eq!(transactions_at(&[1, 2, 3], &[1]), None);
        // a valid payload asked about an offset past its end walks out cleanly
        let payload = bincode::serialize(&Vec::<solana_entry::entry::Entry>::new()).unwrap();
        assert_eq!(transactions_at(&payload, &[]), None);
    }
}
