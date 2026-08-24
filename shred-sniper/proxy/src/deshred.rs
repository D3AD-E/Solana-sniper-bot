use std::{collections::HashSet, hash::Hash, sync::atomic::Ordering};

use itertools::Itertools;
use jito_protos::shredstream::TraceShred;
use log::{debug, warn};
use prost::Message;
use solana_ledger::{
    blockstore::MAX_DATA_SHREDS_PER_SLOT,
    shred::{
        merkle::{Shred, ShredCode},
        ReedSolomonCache, ShredType, Shredder,
    },
};
use solana_metrics::datapoint_warn;
use solana_perf::packet::PacketBatch;
use solana_sdk::clock::{Slot, MAX_PROCESSING_AGE};

use crate::forwarder::ShredMetrics;

#[derive(Default, Debug, Copy, Clone, Eq, PartialEq)]
enum ShredStatus {
    #[default]
    Unknown,
    /// Shred that is **not** marked as [ShredFlags::DATA_COMPLETE_SHRED]
    NotDataComplete,
    /// Shred that is marked as [ShredFlags::DATA_COMPLETE_SHRED]
    DataComplete,
}

/// Tracks per-slot shred information for data shreds
/// Guaranteed to have MAX_DATA_SHREDS_PER_SLOT entries in each Vec
#[derive(Debug)]
pub struct ShredsStateTracker {
    /// Compact status of each data shred for fast iteration.
    data_status: Vec<ShredStatus>,
    /// Data shreds received for the slot (not coding!)
    data_shreds: Vec<Option<Shred>>,
    /// array of bools that track which FEC set indexes have been already recovered
    already_recovered_fec_sets: Vec<bool>,
    /// array of bools that track which data shred indexes have been already deshredded
    already_deshredded: Vec<bool>,
}
impl Default for ShredsStateTracker {
    fn default() -> Self {
        Self {
            data_status: vec![ShredStatus::Unknown; MAX_DATA_SHREDS_PER_SLOT],
            data_shreds: vec![None; MAX_DATA_SHREDS_PER_SLOT],
            already_recovered_fec_sets: vec![false; MAX_DATA_SHREDS_PER_SLOT],
            already_deshredded: vec![false; MAX_DATA_SHREDS_PER_SLOT],
        }
    }
}

/// Returns the number of shreds reconstructed
/// Updates all_shreds with current state, and deshredded_entries with returned values
/// receive shreds per FEC set, attempting to recover the other shreds in the fec set so you do not have to wait until all data shreds have arrived.
/// every time a fec is recovered, scan for neighbouring DATA_COMPLETE_SHRED flags in the shreds, attempting to deserialize into solana entries when there are no missing shreds between the DATA_COMPLETE_SHRED flags.
/// note that an FEC set doesn't necessarily contain DATA_COMPLETE_SHRED in the last shred. when deserializing the bincode data, you must use data between shreds starting at the last DATA_COMPLETE_SHRED (not inclusive) to the next DATA_COMPLETE_SHRED (inclusive)
pub fn reconstruct_shreds(
    packet_batch: PacketBatch,
    all_shreds: &mut ahash::HashMap<
        Slot,
        (
            ahash::HashMap<u32 /* fec_set_index */, HashSet<ComparableShred>>,
            ShredsStateTracker,
        ),
    >,
    slot_fec_indexes_to_iterate: &mut Vec<(Slot, u32)>,
    deshredded_entries: &mut Vec<(Slot, Vec<solana_entry::entry::Entry>, Vec<u8>)>,
    highest_slot_seen: &mut Slot,
    rs_cache: &ReedSolomonCache,
    metrics: &ShredMetrics,
    // pump_only: skip the bincode deserialize for segments with no pump.fun create
    // discriminator. Callers that need every entry (generic consumers, the upstream tests)
    // pass false.
    pump_only: bool,
) -> usize {
    deshredded_entries.clear();
    slot_fec_indexes_to_iterate.clear();
    // ingest all packets
    for packet in packet_batch.iter().filter_map(|p| p.data(..)) {
        match solana_ledger::shred::Shred::new_from_serialized_shred(packet.to_vec())
            .and_then(Shred::try_from)
        {
            Ok(shred) => {
                let slot = shred.common_header().slot;
                let index = shred.index() as usize;
                let fec_set_index = shred.fec_set_index();
                let (all_shreds, state_tracker) = all_shreds.entry(slot).or_default();
                if highest_slot_seen.saturating_sub(SLOT_LOOKBACK) > slot {
                    debug!(
                        "Old shred slot: {slot}, fec_set_index: {fec_set_index}, index: {index}"
                    );
                    continue;
                }
                if state_tracker.already_recovered_fec_sets[fec_set_index as usize]
                    || state_tracker.already_deshredded[index]
                {
                    debug!("Already completed slot: {slot}, fec_set_index: {fec_set_index}, index: {index}");
                    continue;
                }
                let Some(_shred_index) = update_state_tracker(&shred, state_tracker) else {
                    continue;
                };

                all_shreds
                    .entry(fec_set_index)
                    .or_default()
                    .insert(ComparableShred(shred));
                slot_fec_indexes_to_iterate.push((slot, fec_set_index)); // use Vec so we can sort to make sure if any earlier FEC sets have DATA_SHRED_COMPLETE, later entries can use the flag to find the bounds
                *highest_slot_seen = std::cmp::max(*highest_slot_seen, slot);
            }
            Err(e) => {
                if TraceShred::decode(packet).is_ok() {
                    continue;
                }
                warn!("Failed to decode shred. Err: {e:?}");
            }
        }
    }
    slot_fec_indexes_to_iterate.sort_unstable();
    slot_fec_indexes_to_iterate.dedup();

    // try recovering by FEC set
    // already checked if FEC set is completed or deserialized
    let mut total_recovered_count = 0;
    for (slot, fec_set_index) in slot_fec_indexes_to_iterate.iter() {
        let (all_shreds, state_tracker) = all_shreds.entry(*slot).or_default();
        let shreds = all_shreds.entry(*fec_set_index).or_default();
        let (
            num_expected_data_shreds,
            num_expected_coding_shreds,
            num_data_shreds,
            num_coding_shreds,
        ) = get_data_shred_info(shreds);

        // haven't received last data shred, haven't seen any coding shreds, so wait until more arrive
        let min_shreds_needed_to_recover = num_expected_data_shreds as usize;
        if num_expected_data_shreds == 0
            || shreds.len() < min_shreds_needed_to_recover
            || num_data_shreds == num_expected_data_shreds
        {
            continue;
        }

        // try to recover if we have enough shreds in the FEC set
        let merkle_shreds = shreds
            .iter()
            .sorted_by_key(|s| (u8::MAX - s.shred_type() as u8, s.index()))
            .map(|s| s.0.clone())
            .collect_vec();
        let recovered = match solana_ledger::shred::merkle::recover(merkle_shreds, rs_cache) {
            Ok(r) => r, // data shreds followed by code shreds (whatever was missing from to_deshred_payload)
            Err(e) => {
                warn!(
                    "Failed to recover shreds for slot {slot} fec_set_index {fec_set_index}. num_expected_data_shreds: {num_expected_data_shreds}, num_data_shreds: {num_data_shreds} num_expected_coding_shreds: {num_expected_coding_shreds} num_coding_shreds: {num_coding_shreds} Err: {e}",
                );
                continue;
            }
        };

        let mut fec_set_recovered_count = 0;
        for shred in recovered {
            match shred {
                Ok(shred) => {
                    if update_state_tracker(&shred, state_tracker).is_none() {
                        continue; // already seen before in state tracker
                    }
                    // shreds.insert(ComparableShred(shred)); // optional since all data shreds are in state_tracker
                    total_recovered_count += 1;
                    fec_set_recovered_count += 1;
                }
                Err(e) => warn!(
                    "Failed to recover shred for slot {slot}, fec set: {fec_set_index}. Err: {e}"
                ),
            }
        }

        if fec_set_recovered_count > 0 {
            debug!("recovered slot: {slot}, fec_index: {fec_set_index}, recovered count: {fec_set_recovered_count}");
            state_tracker.already_recovered_fec_sets[*fec_set_index as usize] = true;
            shreds.clear();
        }
    }

    // deshred and bincode deserialize
    for (slot, fec_set_index) in slot_fec_indexes_to_iterate.iter() {
        let (_all_shreds, state_tracker) = all_shreds.entry(*slot).or_default();
        let Some((start_data_complete_idx, end_data_complete_idx, unknown_start)) =
            get_indexes(state_tracker, *fec_set_index as usize)
        else {
            continue;
        };
        if unknown_start {
            metrics
                .unknown_start_position_count
                .fetch_add(1, Ordering::Relaxed);
        }

        let to_deshred =
            &state_tracker.data_shreds[start_data_complete_idx..=end_data_complete_idx];
        let deshredded_payload = match Shredder::deshred(
            to_deshred.iter().map(|s| s.as_ref().unwrap().payload()),
        ) {
            Ok(v) => v,
            Err(e) => {
                warn!("slot {slot} failed to deshred slot: {slot}, start_data_complete_idx: {start_data_complete_idx}, end_data_complete_idx: {end_data_complete_idx}. Err: {e}");
                metrics
                    .fec_recovery_error_count
                    .fetch_add(1, Ordering::Relaxed);
                if unknown_start {
                    metrics
                        .unknown_start_position_error_count
                        .fetch_add(1, Ordering::Relaxed);
                }
                continue;
            }
        };

        // Skip the deserialize for segments that cannot hold a launch. Only done when the
        // segment boundaries are known: when `unknown_start` is set, a deserialize failure is
        // how a mis-bounded segment is detected and left for a later retry, so that signal
        // has to be preserved.
        if pump_only && !unknown_start && !may_contain_pump_create(&deshredded_payload) {
            metrics
                .deserialize_skipped_count
                .fetch_add(1, Ordering::Relaxed);
            deshredded_entries.push((*slot, Vec::new(), deshredded_payload));
            to_deshred.iter().for_each(|shred| {
                let Some(shred) = shred.as_ref() else {
                    return;
                };
                state_tracker.already_recovered_fec_sets[shred.fec_set_index() as usize] = true;
                state_tracker.already_deshredded[shred.index() as usize] = true;
            });
            continue;
        }

        // The segment holds a create. Walk it and keep only those transactions rather than
        // materialising every transaction in the segment.
        if pump_only {
            if let Ok(txs) = creates_in_payload(&deshredded_payload) {
                let entries = if txs.is_empty() {
                    Vec::new()
                } else {
                    metrics
                        .txn_count
                        .fetch_add(txs.len() as u64, Ordering::Relaxed);
                    vec![solana_entry::entry::Entry {
                        num_hashes: 0,
                        hash: solana_sdk::hash::Hash::default(),
                        transactions: txs,
                    }]
                };
                deshredded_entries.push((*slot, entries, deshredded_payload));
                to_deshred.iter().for_each(|shred| {
                    let Some(shred) = shred.as_ref() else {
                        return;
                    };
                    state_tracker.already_recovered_fec_sets[shred.fec_set_index() as usize] = true;
                    state_tracker.already_deshredded[shred.index() as usize] = true;
                });
                continue;
            }
        }

        let entries = match bincode::deserialize::<Vec<solana_entry::entry::Entry>>(
            &deshredded_payload,
        ) {
            Ok(entries) => entries,
            Err(e) => {
                debug!(
                        "Failed to deserialize bincode payload of size {} for slot {slot}, start_data_complete_idx: {start_data_complete_idx}, end_data_complete_idx: {end_data_complete_idx}, unknown_start: {unknown_start}. Err: {e}",
                        deshredded_payload.len()
                    );
                metrics
                    .bincode_deserialize_error_count
                    .fetch_add(1, Ordering::Relaxed);
                if unknown_start {
                    metrics
                        .unknown_start_position_error_count
                        .fetch_add(1, Ordering::Relaxed);
                }
                continue;
            }
        };
        metrics
            .entry_count
            .fetch_add(entries.len() as u64, Ordering::Relaxed);
        let txn_count = entries.iter().map(|e| e.transactions.len() as u64).sum();
        metrics.txn_count.fetch_add(txn_count, Ordering::Relaxed);
        debug!(
            "Successfully decoded slot: {slot} start_data_complete_idx: {start_data_complete_idx} end_data_complete_idx: {end_data_complete_idx} with entry count: {}, txn count: {txn_count}",
            entries.len(),
        );

        deshredded_entries.push((*slot, entries, deshredded_payload));
        to_deshred.iter().for_each(|shred| {
            let Some(shred) = shred.as_ref() else {
                return;
            };
            state_tracker.already_recovered_fec_sets[shred.fec_set_index() as usize] = true;
            state_tracker.already_deshredded[shred.index() as usize] = true;
        })
    }

    if all_shreds.len() > MAX_PROCESSING_AGE {
        let slot_threshold = highest_slot_seen.saturating_sub(SLOT_LOOKBACK);
        let mut incomplete_fec_sets = ahash::HashMap::<Slot, Vec<_>>::default();
        let mut incomplete_fec_sets_count = 0;
        all_shreds.retain(|slot, (fec_set_indexes, state_tracker)| {
            if *slot >= slot_threshold {
                return true;
            }

            // count missing fec sets before clearing
            for (fec_set_index, shreds) in fec_set_indexes.iter() {
                if state_tracker.already_recovered_fec_sets[*fec_set_index as usize] {
                    continue;
                }
                let (
                    num_expected_data_shreds,
                    _num_expected_coding_shreds,
                    _num_data_shreds,
                    _num_coding_shreds,
                ) = get_data_shred_info(shreds);

                incomplete_fec_sets_count += 1;
                incomplete_fec_sets
                    .entry(*slot)
                    .and_modify(|fec_set_data| {
                        fec_set_data.push((*fec_set_index, num_expected_data_shreds, shreds.len()))
                    })
                    .or_insert_with(|| {
                        vec![(*fec_set_index, num_expected_data_shreds, shreds.len())]
                    });
            }

            false
        });
        if incomplete_fec_sets_count > 0 {
            incomplete_fec_sets
                .iter_mut()
                .for_each(|(_slot, fec_set_indexes)| fec_set_indexes.sort_unstable());
            datapoint_warn!(
                "shredstream_proxy-deshred_missed_fec_sets",
                (
                    "slot_fec_set_indexes",
                    format!("{:?}", incomplete_fec_sets.iter().sorted().collect_vec()),
                    String
                ),
                ("slot_count", incomplete_fec_sets.len(), i64),
                ("fec_set_count", incomplete_fec_sets_count, i64),
            );
        }
    }

    if total_recovered_count > 0 {
        metrics
            .recovered_count
            .fetch_add(total_recovered_count as u64, Ordering::Relaxed);
    }

    total_recovered_count
}

#[allow(unused)]
fn debug_remaining_shreds(
    all_shreds: &mut ahash::HashMap<
        Slot,
        (
            ahash::HashMap<u32, HashSet<ComparableShred>>,
            ShredsStateTracker,
        ),
    >,
) {
    let mut incomplete_fec_sets = ahash::HashMap::<Slot, Vec<_>>::default();
    let mut incomplete_fec_sets_count = 0;
    all_shreds
        .iter()
        .for_each(|(slot, (fec_set_indexes, state_tracker))| {
            // count missing fec sets before clearing
            for (fec_set_index, shreds) in fec_set_indexes.iter() {
                if state_tracker.already_recovered_fec_sets[*fec_set_index as usize] {
                    continue;
                }
                let (
                    num_expected_data_shreds,
                    _num_expected_coding_shreds,
                    _num_data_shreds,
                    _num_coding_shreds,
                ) = get_data_shred_info(shreds);

                incomplete_fec_sets_count += 1;
                incomplete_fec_sets
                    .entry(*slot)
                    .and_modify(|fec_set_data| {
                        fec_set_data.push((*fec_set_index, num_expected_data_shreds, shreds.len()))
                    })
                    .or_insert_with(|| {
                        vec![(*fec_set_index, num_expected_data_shreds, shreds.len())]
                    });
            }
        });
    incomplete_fec_sets
        .iter_mut()
        .for_each(|(_slot, fec_set_indexes)| fec_set_indexes.sort_unstable());
    println!("{:?}", incomplete_fec_sets.iter().sorted().collect_vec());
}

/// Return the inclusive range of shreds that constitute one complete segment: [0+ NotDataComplete, DataComplete]
/// Rules:
/// * A segment **ends** at the first `DataComplete` *at or after* `index`.
/// * It **starts** one position after the previous `DataComplete`, or at the beginning of the vector if there is none.
/// * If an `Unknown` is seen while searching towards the right, the segment is discarded and `None` is returned.
/// * We allow `Unknown` towards the left since sometimes entire FEC sets are not sent out
fn get_indexes(
    tracker: &ShredsStateTracker,
    index: usize,
) -> Option<(
    usize, /* start_data_complete_idx */
    usize, /* end_data_complete_idx */
    bool,  /* unknown start index */
)> {
    if index >= tracker.data_status.len() {
        return None;
    }

    // find the right boundary (first DataComplete ≥ index)
    let mut end = index;
    while end < tracker.data_status.len() {
        if tracker.already_deshredded[end] {
            return None;
        }
        match &tracker.data_status[end] {
            ShredStatus::Unknown => return None,
            ShredStatus::DataComplete => break,
            ShredStatus::NotDataComplete => end += 1,
        }
    }
    if end == tracker.data_status.len() {
        return None; // never saw a DataComplete
    }

    if end == 0 {
        return Some((0, 0, false)); // the vec *starts* with DataComplete
    }
    if index == 0 {
        return Some((0, end, false));
    }

    // find the left boundary (prev DataComplete + 1)
    let mut start = index;
    let mut next = start - 1;
    loop {
        match tracker.data_status[next] {
            ShredStatus::NotDataComplete => {
                if tracker.already_deshredded[next] {
                    return None; // already covered by some other iteration
                }
                if next == 0 {
                    return Some((0, end, false)); // no earlier DataComplete
                }
                start = next;
                next -= 1;
            }
            ShredStatus::DataComplete => return Some((start, end, false)),
            ShredStatus::Unknown => return Some((start, end, true)), // sometimes we don't have the previous starting shreds, make best guess
        }
    }
}

/// Upon receiving a new shred (either from recovery or receiving a UDP packet), update the state tracker
/// Returns shred index on new insert, None if already exists
fn update_state_tracker(shred: &Shred, state_tracker: &mut ShredsStateTracker) -> Option<usize> {
    let index = shred.index() as usize;
    if state_tracker.already_recovered_fec_sets[shred.fec_set_index() as usize] {
        return None;
    }
    if shred.shred_type() == ShredType::Data
        && (state_tracker.data_shreds[index].is_some()
            || !matches!(state_tracker.data_status[index], ShredStatus::Unknown))
    {
        return None;
    }
    if let Shred::ShredData(s) = &shred {
        state_tracker.data_shreds[index] = Some(shred.clone());
        if s.data_complete() || s.last_in_slot() {
            state_tracker.data_status[index] = ShredStatus::DataComplete;
        } else {
            state_tracker.data_status[index] = ShredStatus::NotDataComplete;
        }
    };
    Some(index)
}


/// True when the deshredded payload might contain a pump.fun create.
///
/// The payload here is already assembled and contiguous, so an instruction's 8 byte
/// discriminator cannot be split the way a pubkey could be split across raw shreds — which
/// is what made the old shred-level prefilter drop launches. Deserializing a segment costs
/// ~54us and only about one segment in six hundred holds a create, so this scan pays for
/// itself many times over.
#[inline]
fn may_contain_pump_create(payload: &[u8]) -> bool {
    static CREATE_V2: std::sync::OnceLock<memchr::memmem::Finder<'static>> =
        std::sync::OnceLock::new();
    static CREATE: std::sync::OnceLock<memchr::memmem::Finder<'static>> =
        std::sync::OnceLock::new();
    let v2 = CREATE_V2.get_or_init(|| memchr::memmem::Finder::new(&sniper::pumpfun::DISC_CREATE_V2));
    let v1 = CREATE.get_or_init(|| memchr::memmem::Finder::new(&sniper::pumpfun::DISC_CREATE));
    v2.find(payload).is_some() || v1.find(payload).is_some()
}


/// Streams the segment and returns only the transactions that are pump.fun creates.
///
/// A full `Vec<Entry>` deserialize allocates a transaction — with its signatures, account
/// keys and instruction data — for every transaction in the segment, and costs ~44us. The
/// discriminator scan already tells us the offset of the *last* create in the payload, so
/// the walk can stop there instead of decoding the tail. Every create is still seen: the
/// stop point is the last hit, not the first.
///
/// Returns `Err(())` if the payload does not walk cleanly, and the caller falls back to the
/// ordinary deserialize so a malformed segment is handled exactly as before.
fn creates_in_payload(payload: &[u8]) -> Result<Vec<solana_sdk::transaction::VersionedTransaction>, ()> {
    let last_hit = last_create_offset(payload).ok_or(())?;
    let mut cur = std::io::Cursor::new(payload);
    let entry_count: u64 = bincode::deserialize_from(&mut cur).map_err(|_| ())?;

    let mut found = Vec::new();
    for _ in 0..entry_count {
        if cur.position() as usize > last_hit {
            break;
        }
        let _num_hashes: u64 = bincode::deserialize_from(&mut cur).map_err(|_| ())?;
        let mut hash = [0u8; 32];
        std::io::Read::read_exact(&mut cur, &mut hash).map_err(|_| ())?;
        let tx_count: u64 = bincode::deserialize_from(&mut cur).map_err(|_| ())?;
        for _ in 0..tx_count {
            let tx: solana_sdk::transaction::VersionedTransaction =
                bincode::deserialize_from(&mut cur).map_err(|_| ())?;
            if sniper::pumpfun::parse_create(&tx).is_some() {
                found.push(tx);
            }
            if cur.position() as usize > last_hit {
                return Ok(found);
            }
        }
    }
    Ok(found)
}

/// Offset of the last create discriminator in the payload.
#[inline]
fn last_create_offset(payload: &[u8]) -> Option<usize> {
    let v2 = memchr::memmem::rfind(payload, &sniper::pumpfun::DISC_CREATE_V2);
    let v1 = memchr::memmem::rfind(payload, &sniper::pumpfun::DISC_CREATE);
    match (v2, v1) {
        (Some(a), Some(b)) => Some(a.max(b)),
        (Some(a), None) => Some(a),
        (None, Some(b)) => Some(b),
        (None, None) => None,
    }
}

const SLOT_LOOKBACK: Slot = 50;

/// check if we can reconstruct (having minimum number of data + coding shreds)
fn get_data_shred_info(
    shreds: &HashSet<ComparableShred>,
) -> (
    u16, /* num_expected_data_shreds */
    u16, /* num_expected_coding_shreds */
    u16, /* num_data_shreds */
    u16, /* num_coding_shreds */
) {
    let mut num_expected_data_shreds = 0;
    let mut num_expected_coding_shreds = 0;
    let mut num_data_shreds = 0;
    let mut num_coding_shreds = 0;
    for shred in shreds {
        match &shred.0 {
            Shred::ShredCode(s) => {
                num_coding_shreds += 1;
                num_expected_data_shreds = s.coding_header.num_data_shreds;
                num_expected_coding_shreds = s.coding_header.num_coding_shreds;
            }
            Shred::ShredData(s) => {
                num_data_shreds += 1;
                if num_expected_data_shreds == 0 && (s.data_complete() || s.last_in_slot()) {
                    num_expected_data_shreds =
                        (shred.0.index() - shred.0.fec_set_index()) as u16 + 1;
                }
            }
        }
    }
    (
        num_expected_data_shreds,
        num_expected_coding_shreds,
        num_data_shreds,
        num_coding_shreds,
    )
}

/// Issue: datashred equality comparison is wrong due to data size being smaller than the 1203 bytes allocated
#[derive(Clone, Debug, Eq)]
pub struct ComparableShred(Shred);

impl std::ops::Deref for ComparableShred {
    type Target = Shred;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl Hash for ComparableShred {
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        match &self.0 {
            Shred::ShredCode(s) => {
                s.common_header.hash(state);
                s.coding_header.hash(state);
            }
            Shred::ShredData(s) => {
                s.common_header.hash(state);
                s.data_header.hash(state);
            }
        }
    }
}

impl PartialEq for ComparableShred {
    // Custom comparison to avoid random bytes that are part of payload
    fn eq(&self, other: &Self) -> bool {
        match &self.0 {
            Shred::ShredCode(s1) => match &other.0 {
                Shred::ShredCode(s2) => {
                    let solana_ledger::shred::ShredVariant::MerkleCode {
                        proof_size,
                        chained: _,
                        resigned,
                    } = s1.common_header.shred_variant
                    else {
                        return false;
                    };

                    // see https://github.com/jito-foundation/jito-solana/blob/d6c73374e3b4f863436e4b7d4d1ce5eea01cd262/ledger/src/shred/merkle.rs#L346, and re-add the proof component
                    let comparison_len =
                        <ShredCode as solana_ledger::shred::traits::Shred>::SIZE_OF_PAYLOAD
                            .saturating_sub(
                                usize::from(proof_size)
                                    * solana_ledger::shred::merkle::SIZE_OF_MERKLE_PROOF_ENTRY
                                    + if resigned {
                                        solana_ledger::shred::SIZE_OF_SIGNATURE
                                    } else {
                                        0
                                    },
                            );

                    s1.coding_header == s2.coding_header
                        && s1.common_header == s2.common_header
                        && s1.payload[..comparison_len] == s2.payload[..comparison_len]
                }
                Shred::ShredData(_) => false,
            },
            Shred::ShredData(s1) => match &other.0 {
                Shred::ShredCode(_) => false,
                Shred::ShredData(s2) => {
                    let Ok(s1_data) = solana_ledger::shred::layout::get_data(self.payload()) else {
                        return false;
                    };
                    let Ok(s2_data) = solana_ledger::shred::layout::get_data(other.payload())
                    else {
                        return false;
                    };
                    s1.data_header == s2.data_header
                        && s1.common_header == s2.common_header
                        && s1_data == s2_data
                }
            },
        }
    }
}
#[cfg(test)]
mod tests {
    use std::{
        collections::{hash_map::Entry, HashSet},
        io::{Read, Write},
        net::UdpSocket,
        sync::Arc,
    };

    use borsh::BorshDeserialize;
    use itertools::Itertools;
    use rand::Rng;
    use solana_ledger::{
        blockstore::make_slot_entries_with_transactions,
        shred::{merkle::Shred, ProcessShredsStats, ReedSolomonCache, ShredCommonHeader, Shredder},
    };
    use solana_perf::packet::{Packet, PacketBatch};
    use solana_sdk::{clock::Slot, hash::Hash, signature::Keypair};

    use crate::{
        deshred::{reconstruct_shreds, ComparableShred},
        forwarder::ShredMetrics,
    };

    /// For serializing packets to disk
    #[derive(borsh::BorshSerialize, borsh::BorshDeserialize, PartialEq, Debug)]
    struct Packets {
        pub packets: Vec<Vec<u8>>,
    }

    #[allow(unused)]
    fn listen_and_write_shreds() -> std::io::Result<()> {
        let socket = UdpSocket::bind("127.0.0.1:5000")?;
        println!("Listening on {}", socket.local_addr()?);

        let mut map = ahash::HashMap::<usize, usize>::default();
        let mut buf = [0u8; 1500];
        let mut vec = Packets {
            packets: Vec::new(),
        };

        let mut i = 0;
        loop {
            i += 1;
            match socket.recv_from(&mut buf) {
                Ok((amt, _src)) => {
                    vec.packets.push(buf[..amt].to_vec());
                    match map.entry(amt) {
                        Entry::Occupied(mut e) => *e.get_mut() += 1,
                        Entry::Vacant(e) => {
                            e.insert(1);
                        }
                    }
                    *map.get_mut(&amt).unwrap_or(&mut 0) += 1;
                }
                Err(e) => {
                    eprintln!("Error receiving data: {}", e);
                }
            }
            if i % 50000 == 0 {
                dbg!(&map);
                // size 1203 are data shreds: https://github.com/jito-foundation/jito-solana/blob/1742826fca975bd6d17daa5693abda861bbd2adf/ledger/src/shred/merkle.rs#L42
                // size 1228 are coding shreds: https://github.com/jito-foundation/jito-solana/blob/1742826fca975bd6d17daa5693abda861bbd2adf/ledger/src/shred/shred_code.rs#L16
                let mut file = std::fs::File::create("serialized_shreds.bin")?;
                file.write_all(&borsh::to_vec(&vec)?)?;
                return Ok(());
            }
        }
    }

    #[test]
    fn test_reconstruct_live_shreds() {
        let packets = {
            let mut file = std::fs::File::open("../bins/serialized_shreds.bin").unwrap();
            let mut buffer = Vec::new();
            file.read_to_end(&mut buffer).unwrap();
            Packets::try_from_slice(&buffer).unwrap()
        };
        assert_eq!(packets.packets.len(), 50_000);

        let shreds = packets
            .packets
            .iter()
            .filter_map(|p| Shred::from_payload(p.clone()).ok())
            .collect::<Vec<_>>();
        assert_eq!(shreds.len(), 49989);

        let unique_shreds = packets
            .packets
            .iter()
            .filter_map(|p| Shred::from_payload(p.clone()).ok().map(ComparableShred))
            .collect::<HashSet<ComparableShred>>();
        assert_eq!(unique_shreds.len(), 44900);

        let unique_slot_fec_shreds = packets
            .packets
            .iter()
            .filter_map(|p| {
                Shred::from_payload(p.clone())
                    .ok()
                    .map(|s| *s.common_header())
            })
            .collect::<HashSet<ShredCommonHeader>>();
        assert_eq!(unique_slot_fec_shreds.len(), 44900);

        let rs_cache = ReedSolomonCache::default();
        let metrics = Arc::new(ShredMetrics::default());

        // Test 1: all shreds provided
        let mut all_shreds = ahash::HashMap::default();
        let mut slot_fec_indexes_to_iterate: Vec<(Slot, u32)> = Vec::new();
        let mut deshredded_entries = Vec::new();
        let mut highest_slot_seen = 0;
        let recovered_count = reconstruct_shreds(
            PacketBatch::new(
                packets
                    .packets
                    .iter()
                    .map(|x| {
                        let mut packet = Packet::default();
                        packet.buffer_mut()[..x.len()].copy_from_slice(x);
                        packet.meta_mut().size = x.len();
                        packet
                    })
                    .collect_vec(),
            ),
            &mut all_shreds,
            &mut slot_fec_indexes_to_iterate,
            &mut deshredded_entries,
            &mut highest_slot_seen,
            &rs_cache,
            &metrics,
            false,
        );

        // debug_to_disk(&mut deshredded_entries);
        assert!(recovered_count < deshredded_entries.len());
        assert_eq!(
            deshredded_entries
                .iter()
                .map(|(_slot, entries, _entries_bytes)| entries.len())
                .sum::<usize>(),
            13580
        );
        assert_eq!(all_shreds.len(), 30);

        let slot_to_entry = deshredded_entries
            .iter()
            .into_group_map_by(|(slot, _entries, _entries_bytes)| *slot);
        // slot_to_entry
        //     .iter()
        //     .sorted_by_key(|(slot, _)| *slot)
        //     .for_each(|(slot, entry)| {
        //         println!(
        //             "slot {slot} entry count: {:?}, txn count: {}",
        //             entry.len(),
        //             entry
        //                 .iter()
        //                 .map(|(_slot, entry)| entry.transactions.len())
        //                 .sum::<usize>()
        //         );
        //     });
        assert_eq!(slot_to_entry.len(), 29);

        // Test 2: 33% of shreds missing
        let mut all_shreds = ahash::HashMap::default();
        let mut slot_fec_indexes_to_iterate: Vec<(Slot, u32)> = Vec::new();
        let mut deshredded_entries = Vec::new();
        let mut highest_slot_seen = 0;
        let recovered_count = reconstruct_shreds(
            PacketBatch::new(
                packets
                    .packets
                    .iter()
                    .enumerate()
                    .filter(|(index, _)| (index + 1) % 3 != 0)
                    .map(|(_i, x)| {
                        let mut packet = Packet::default();
                        packet.buffer_mut()[..x.len()].copy_from_slice(x);
                        packet.meta_mut().size = x.len();
                        packet
                    })
                    .collect_vec(),
            ),
            &mut all_shreds,
            &mut slot_fec_indexes_to_iterate,
            &mut deshredded_entries,
            &mut highest_slot_seen,
            &rs_cache,
            &metrics,
            false,
        );

        // debug_to_disk(&deshredded_entries, "new.txt");
        assert!(recovered_count > (deshredded_entries.len() / 4));
        assert_eq!(
            deshredded_entries
                .iter()
                .map(|(_slot, entries, _entries_bytes)| entries.len())
                .sum::<usize>(),
            13580
        );
        assert!(all_shreds.len() > 15);

        let slot_to_entry = deshredded_entries
            .iter()
            .into_group_map_by(|(slot, _entries, _entries_bytes)| *slot);
        assert_eq!(slot_to_entry.len(), 29);
    }

    /// Helper function to compare all shred output
    #[allow(unused)]
    fn debug_to_disk(
        deshredded_entries: &[(Slot, Vec<solana_entry::entry::Entry>, Vec<u8>)],
        filepath: &str,
    ) {
        let entries = deshredded_entries
            .iter()
            .map(|(slot, entries, _entries_bytes)| (slot, entries))
            .into_group_map_by(|(slot, _entries)| *slot)
            .into_iter()
            .map(|(key, values)| {
                (
                    key,
                    values.into_iter().fold(Vec::new(), |mut acc, (_, v)| {
                        acc.extend(v);
                        acc
                    }),
                )
            })
            .map(|(slot, entries)| {
                let mut vec = entries
                    .iter()
                    .flat_map(|x| x.transactions.iter())
                    .map(|x| x.signatures[0])
                    .collect::<Vec<_>>();
                vec.sort();
                vec.dedup();
                (slot, vec)
            })
            .sorted_by_key(|x| x.0)
            .dedup_by(|lhs, rhs| lhs.0 == rhs.0)
            .collect_vec();
        let mut file = std::fs::File::create(filepath).unwrap();
        write!(file, "entries: {:#?}", &entries).unwrap();
    }

    #[test]
    /// Test if DATA_COMPLETE_SHRED across multiple FEC sets is handled correctly
    fn test_reconstruct_live_data_complete_shred() {
        let packets = {
            let mut file =
                std::fs::File::open("../bins/serialized_shreds_data_complete_test.bin").unwrap();
            let mut buffer = Vec::new();
            file.read_to_end(&mut buffer).unwrap();
            Packets::try_from_slice(&buffer).unwrap()
        };
        assert_eq!(packets.packets.len(), 150_000);

        let shreds = packets
            .packets
            .iter()
            .filter_map(|p| Shred::from_payload(p.clone()).ok())
            .collect::<Vec<_>>();
        assert_eq!(shreds.len(), 149977);

        let unique_shreds = packets
            .packets
            .iter()
            .filter_map(|p| Shred::from_payload(p.clone()).ok().map(ComparableShred))
            .collect::<HashSet<ComparableShred>>();
        assert_eq!(unique_shreds.len(), 109221);

        let unique_slot_fec_shreds = packets
            .packets
            .iter()
            .filter_map(|p| {
                Shred::from_payload(p.clone())
                    .ok()
                    .map(|s| *s.common_header())
            })
            .collect::<HashSet<ShredCommonHeader>>();
        assert_eq!(unique_slot_fec_shreds.len(), 109221);

        let rs_cache = ReedSolomonCache::default();
        let metrics = Arc::new(ShredMetrics::default());

        // Test 1: all shreds provided
        let mut all_shreds = ahash::HashMap::default();
        let mut slot_fec_indexes_to_iterate: Vec<(Slot, u32)> = Vec::new();
        let mut deshredded_entries = Vec::new();
        let mut highest_slot_seen = 0;
        let recovered_count = reconstruct_shreds(
            PacketBatch::new(
                packets
                    .packets
                    .iter()
                    .map(|x| {
                        let mut packet = Packet::default();
                        packet.buffer_mut()[..x.len()].copy_from_slice(x);
                        packet.meta_mut().size = x.len();
                        packet
                    })
                    .collect_vec(),
            ),
            &mut all_shreds,
            &mut slot_fec_indexes_to_iterate,
            &mut deshredded_entries,
            &mut highest_slot_seen,
            &rs_cache,
            &metrics,
            false,
        );

        // debug_to_disk(&mut deshredded_entries);
        assert!(recovered_count < deshredded_entries.len());
        assert_eq!(
            deshredded_entries
                .iter()
                .map(|(_slot, entries, _entries_bytes)| entries.len())
                .sum::<usize>(),
            43170
        );
        assert_eq!(all_shreds.len(), 61);

        let slot_to_entry = deshredded_entries
            .iter()
            .into_group_map_by(|(slot, _entries, _entries_bytes)| *slot);
        // slot_to_entry
        //     .iter()
        //     .sorted_by_key(|(slot, _)| *slot)
        //     .for_each(|(slot, entry)| {
        //         println!(
        //             "slot {slot} entry count: {:?}, txn count: {}",
        //             entry.len(),
        //             entry
        //                 .iter()
        //                 .map(|(_slot, entry)| entry.transactions.len())
        //                 .sum::<usize>()
        //         );
        //     });
        assert_eq!(slot_to_entry.len(), 61);

        // Test 2: 33% of shreds missing
        let mut all_shreds = ahash::HashMap::default();
        let mut slot_fec_indexes_to_iterate: Vec<(Slot, u32)> = Vec::new();
        let mut deshredded_entries = Vec::new();
        let mut highest_slot_seen = 0;
        let recovered_count = reconstruct_shreds(
            PacketBatch::new(
                packets
                    .packets
                    .iter()
                    .enumerate()
                    .filter(|(index, _)| (index + 1) % 3 != 0)
                    .map(|(_i, x)| {
                        let mut packet = Packet::default();
                        packet.buffer_mut()[..x.len()].copy_from_slice(x);
                        packet.meta_mut().size = x.len();
                        packet
                    })
                    .collect_vec(),
            ),
            &mut all_shreds,
            &mut slot_fec_indexes_to_iterate,
            &mut deshredded_entries,
            &mut highest_slot_seen,
            &rs_cache,
            &metrics,
            false,
        );

        // debug_to_disk(&deshredded_entries, "new.txt");
        assert!(recovered_count > (deshredded_entries.len() / 4));
        assert_eq!(
            deshredded_entries
                .iter()
                .map(|(_slot, entries, _entries_bytes)| entries.len())
                .sum::<usize>(),
            43170
        );
        assert!(all_shreds.len() > 15);

        let slot_to_entry = deshredded_entries
            .iter()
            .into_group_map_by(|(slot, _entries, _entries_bytes)| *slot);
        assert_eq!(slot_to_entry.len(), 61);
    }

    #[test]
    fn test_recover_shreds() {
        let mut rng = rand::thread_rng();
        let slot = 11_111;
        let leader_keypair = Arc::new(Keypair::new());
        let reed_solomon_cache = ReedSolomonCache::default();
        let shredder = Shredder::new(slot, slot - 1, 0, 0).unwrap();
        let chained_merkle_root = Some(Hash::new_from_array(rng.gen()));
        let num_entry_groups = 10;
        let num_entries = 10;
        let mut entries = Vec::new();
        let mut data_shreds = Vec::new();
        let mut coding_shreds = Vec::new();

        let mut index = 0;
        (0..num_entry_groups).for_each(|_i| {
            let _entries = make_slot_entries_with_transactions(num_entries);
            let (_data_shreds, _coding_shreds) = shredder.entries_to_shreds(
                &leader_keypair,
                _entries.as_slice(),
                true,
                chained_merkle_root,
                index as u32, // next_shred_index
                index as u32, // next_code_index,
                true,         // merkle_variant
                &reed_solomon_cache,
                &mut ProcessShredsStats::default(),
            );
            index += _data_shreds.len();
            entries.extend(_entries);
            data_shreds.extend(_data_shreds);
            coding_shreds.extend(_coding_shreds);
        });

        let packets = data_shreds
            .iter()
            .chain(coding_shreds.iter())
            .map(|s| {
                let mut p = Packet::default();
                s.copy_to_packet(&mut p);
                p
            })
            .collect_vec();
        assert_eq!(data_shreds.len(), 320);
        assert_eq!(
            data_shreds
                .iter()
                .map(|s| s.fec_set_index())
                .dedup()
                .count(),
            num_entry_groups
        );

        let metrics = Arc::new(ShredMetrics::default());
        let rs_cache = ReedSolomonCache::default();

        // Test 1: all shreds provided
        let mut all_shreds = ahash::HashMap::default();
        let mut slot_fec_indexes_to_iterate: Vec<(Slot, u32)> = Vec::new();
        let mut deshredded_entries = Vec::new();
        let mut highest_slot_seen = 0;
        let recovered_count = reconstruct_shreds(
            PacketBatch::new(packets.clone()),
            &mut all_shreds,
            &mut slot_fec_indexes_to_iterate,
            &mut deshredded_entries,
            &mut highest_slot_seen,
            &rs_cache,
            &metrics,
            false,
        );
        assert_eq!(recovered_count, 0);
        assert_eq!(
            deshredded_entries
                .iter()
                .map(|(_slot, entries, _entries_bytes)| entries.len())
                .sum::<usize>(),
            entries.len()
        );
        assert_eq!(
            all_shreds.len(),
            1, // slot 11111
        );

        // Test 2: 33% of shreds missing
        let mut all_shreds = ahash::HashMap::default();
        let mut slot_fec_indexes_to_iterate: Vec<(Slot, u32)> = Vec::new();
        let mut deshredded_entries = Vec::new();
        let mut highest_slot_seen = 0;
        let recovered_count = reconstruct_shreds(
            PacketBatch::new(
                packets
                    .iter()
                    .enumerate()
                    .filter(|(index, _)| (index + 1) % 3 != 0)
                    .map(|(_i, p)| p.clone())
                    .collect(),
            ),
            &mut all_shreds,
            &mut slot_fec_indexes_to_iterate,
            &mut deshredded_entries,
            &mut highest_slot_seen,
            &rs_cache,
            &metrics,
            false,
        );
        assert!(recovered_count > 0);
        assert_eq!(
            deshredded_entries
                .iter()
                .map(|(_slot, entries, _entries_bytes)| entries.len())
                .sum::<usize>(),
            entries.len()
        );
        assert_eq!(
            all_shreds.len(),
            1, // slot 11111
        );
    }
    /// Replays recorded live shreds and counts the pump.fun launches that come out.
    ///
    /// This is the regression test for the two bugs that hid launches:
    ///
    /// * entries used to be bincode-deserialized per FEC set, so only the set holding the
    ///   start of the `Vec<Entry>` decoded and any transaction straddling a set boundary was
    ///   lost. Decoding now walks DATA_COMPLETE boundaries across FEC sets, which is why the
    ///   replay yields many more decoded segments than there are slots.
    /// * detection used to scan raw shred payloads for a 32 byte pubkey, which
    ///   false-negatives when the key straddles two shreds or lives only in coding shreds.
    ///   It now runs on reconstructed transactions.

    /// Where the time actually goes between a shred arriving and a create being parsed.
    ///
    /// `cargo test --release -p jito-shredstream-proxy bench_deshred -- --nocapture`
    #[test]
    fn bench_deshred_stages() {
        let Ok(buffer) = std::fs::read("../bins/serialized_shreds.bin") else {
            eprintln!("skipping: ../bins/serialized_shreds.bin not present");
            return;
        };
        let packets = Packets::try_from_slice(&buffer).unwrap();

        // 1. shred parse, per packet
        let t = std::time::Instant::now();
        let mut parsed = 0usize;
        for p in &packets.packets {
            if solana_ledger::shred::Shred::new_from_serialized_shred(p.clone())
                .and_then(Shred::try_from)
                .is_ok()
            {
                parsed += 1;
            }
        }
        let per_shred = t.elapsed().as_nanos() / packets.packets.len() as u128;
        println!("shred parse:            {per_shred:>8}ns per shred ({parsed} parsed)");

        // 2. the whole reconstruct, including FEC recovery and bincode
        let rs_cache = ReedSolomonCache::default();
        let metrics = Arc::new(ShredMetrics::default());
        let mut all_shreds = ahash::HashMap::default();
        let mut idx: Vec<(Slot, u32)> = Vec::new();
        let mut out = Vec::new();
        let mut highest = 0;
        let batch = PacketBatch::new(
            packets
                .packets
                .iter()
                .map(|x| {
                    let mut packet = Packet::default();
                    packet.buffer_mut()[..x.len()].copy_from_slice(x);
                    packet.meta_mut().size = x.len();
                    packet
                })
                .collect_vec(),
        );
        let t = std::time::Instant::now();
        reconstruct_shreds(
            batch,
            &mut all_shreds,
            &mut idx,
            &mut out,
            &mut highest,
            &rs_cache,
            &metrics,
            true,
        );
        let total = t.elapsed();
        let segments = out.len();
        println!(
            "reconstruct_shreds:     {:>8}us total for {} packets -> {} segments ({:.0}us per segment)",
            total.as_micros(),
            packets.packets.len(),
            segments,
            total.as_micros() as f64 / segments as f64
        );

        // 3. bincode deserialize alone, per segment
        let payloads: Vec<Vec<u8>> = out.iter().map(|(_, _, bytes)| bytes.clone()).collect();
        let t = std::time::Instant::now();
        let mut entries = 0usize;
        for p in &payloads {
            if let Ok(e) = bincode::deserialize::<Vec<solana_entry::entry::Entry>>(p) {
                entries += e.len();
            }
        }
        let de = t.elapsed();
        println!(
            "bincode deserialize:    {:>8}us for {} segments ({:.0}us per segment, {} entries)",
            de.as_micros(),
            payloads.len(),
            de.as_micros() as f64 / payloads.len() as f64,
            entries
        );

        // 4. scanning the same payloads for the create discriminator instead
        let t = std::time::Instant::now();
        let mut hits = 0usize;
        for p in &payloads {
            if contains_create_discriminator(p) {
                hits += 1;
            }
        }
        let scan = t.elapsed();
        println!(
            "discriminator prescan:  {:>8}us for {} segments ({:.2}us per segment, {} hits)",
            scan.as_micros(),
            payloads.len(),
            scan.as_micros() as f64 / payloads.len() as f64,
            hits
        );

        // 5. parse_create over every decoded transaction
        let txs: Vec<_> = out
            .iter()
            .flat_map(|(_, entries, _)| entries.iter())
            .flat_map(|e| e.transactions.iter())
            .collect();
        let t = std::time::Instant::now();
        let mut found = 0usize;
        for tx in &txs {
            if sniper::pumpfun::parse_create(tx).is_some() {
                found += 1;
            }
        }
        let pc = t.elapsed();
        println!(
            "parse_create:           {:>8}ns per transaction ({} txs, {} creates)",
            pc.as_nanos() / txs.len().max(1) as u128,
            txs.len(),
            found
        );
    }

    /// The 8 byte anchor discriminator of a pump.fun create, searched in a *contiguous*
    /// deshredded payload. Unlike the old prefilter, which scanned raw shred payloads and
    /// missed keys that straddled a shred boundary, the payload here is already assembled,
    /// so an instruction's data cannot be split.
    fn contains_create_discriminator(payload: &[u8]) -> bool {
        super::may_contain_pump_create(payload)
    }


    /// The streaming walk must find exactly the creates a full deserialize finds. This is the
    /// guarantee that the fast path cannot silently drop a launch.
    #[test]
    fn streaming_walk_agrees_with_full_deserialize() {
        let Ok(buffer) = std::fs::read("../bins/serialized_shreds.bin") else {
            eprintln!("skipping: ../bins/serialized_shreds.bin not present");
            return;
        };
        let packets = Packets::try_from_slice(&buffer).unwrap();
        let rs_cache = ReedSolomonCache::default();
        let metrics = Arc::new(ShredMetrics::default());
        let mut all_shreds = ahash::HashMap::default();
        let mut idx: Vec<(Slot, u32)> = Vec::new();
        let mut out = Vec::new();
        let mut highest = 0;

        // full path: every entry decoded
        reconstruct_shreds(
            PacketBatch::new(
                packets
                    .packets
                    .iter()
                    .map(|x| {
                        let mut packet = Packet::default();
                        packet.buffer_mut()[..x.len()].copy_from_slice(x);
                        packet.meta_mut().size = x.len();
                        packet
                    })
                    .collect_vec(),
            ),
            &mut all_shreds,
            &mut idx,
            &mut out,
            &mut highest,
            &rs_cache,
            &metrics,
            false,
        );

        let mut checked = 0usize;
        let mut creates_seen = 0usize;
        for (_slot, entries, payload) in &out {
            let full: Vec<_> = entries
                .iter()
                .flat_map(|e| e.transactions.iter())
                .filter_map(sniper::pumpfun::parse_create)
                .map(|i| i.mint)
                .collect();

            let fast: Vec<_> = if super::may_contain_pump_create(payload) {
                super::creates_in_payload(payload)
                    .expect("payload that walks under bincode must walk here too")
                    .iter()
                    .filter_map(sniper::pumpfun::parse_create)
                    .map(|i| i.mint)
                    .collect()
            } else {
                Vec::new()
            };

            assert_eq!(full, fast, "streaming walk disagreed on a segment");
            creates_seen += full.len();
            checked += 1;
        }
        println!("compared {checked} segments, {creates_seen} creates, identical");
        assert!(checked > 0);
    }

    #[test]
    fn test_pump_creates_recovered_from_live_shreds() {
        let Ok(buffer) = std::fs::read("../bins/serialized_shreds.bin") else {
            eprintln!("skipping: ../bins/serialized_shreds.bin not present (git lfs)");
            return;
        };
        let packets = Packets::try_from_slice(&buffer).unwrap();

        let rs_cache = ReedSolomonCache::default();
        let metrics = Arc::new(ShredMetrics::default());
        let mut all_shreds = ahash::HashMap::default();
        let mut slot_fec_indexes_to_iterate: Vec<(Slot, u32)> = Vec::new();
        let mut deshredded_entries = Vec::new();
        let mut highest_slot_seen = 0;

        reconstruct_shreds(
            PacketBatch::new(
                packets
                    .packets
                    .iter()
                    .map(|x| {
                        let mut packet = Packet::default();
                        packet.buffer_mut()[..x.len()].copy_from_slice(x);
                        packet.meta_mut().size = x.len();
                        packet
                    })
                    .collect_vec(),
            ),
            &mut all_shreds,
            &mut slot_fec_indexes_to_iterate,
            &mut deshredded_entries,
            &mut highest_slot_seen,
            &rs_cache,
            &metrics,
            true,
        );

        let mut txn_count = 0usize;
        let mut creates = 0usize;
        let mut mints = HashSet::new();
        for (_slot, entries, _bytes) in &deshredded_entries {
            for entry in entries {
                txn_count += entry.transactions.len();
                for tx in &entry.transactions {
                    if let Some(info) = sniper::pumpfun::parse_create(tx) {
                        creates += 1;
                        mints.insert(info.mint);
                    }
                }
            }
        }

        let slots = deshredded_entries
            .iter()
            .map(|(slot, _, _)| *slot)
            .collect::<HashSet<_>>();
        println!(
            "replay: {} segments over {} slots, {txn_count} transactions, {creates} pump creates, {} distinct mints",
            deshredded_entries.len(),
            slots.len(),
            mints.len(),
        );

        assert!(txn_count > 0, "replay must decode transactions");
        // one FEC set carries ~32 data shreds, so a slot spans many of them: decoding more
        // segments than slots is only possible when entries are assembled across FEC sets
        assert!(
            deshredded_entries.len() > slots.len() * 4,
            "expected segments well beyond each slot's first FEC set, got {} segments over {} slots",
            deshredded_entries.len(),
            slots.len()
        );
        assert_eq!(creates, mints.len(), "each create should be seen once");
    }
}
#[cfg(test)]
mod get_indexes_tests {
    use super::{get_indexes, ShredStatus, ShredsStateTracker};

    fn make_test_statustracker(statuses: &[ShredStatus]) -> ShredsStateTracker {
        let mut tracker = ShredsStateTracker::default();
        tracker.data_status[..statuses.len()].copy_from_slice(statuses);
        tracker
    }

    #[test]
    fn start_at_index_zero() {
        let s = [
            ShredStatus::NotDataComplete,
            ShredStatus::NotDataComplete,
            ShredStatus::DataComplete,
        ];
        let tracker = make_test_statustracker(&s);
        assert_eq!(get_indexes(&tracker, 0), Some((0, 2, false)));

        let s = [
            ShredStatus::DataComplete,
            ShredStatus::NotDataComplete,
            ShredStatus::DataComplete,
        ];
        let tracker = make_test_statustracker(&s);
        assert_eq!(get_indexes(&tracker, 0), Some((0, 0, false)));

        let s = [
            ShredStatus::Unknown,
            ShredStatus::NotDataComplete,
            ShredStatus::DataComplete,
        ];
        let tracker = make_test_statustracker(&s);
        assert_eq!(get_indexes(&tracker, 0), None);
    }

    #[test]
    fn start_just_after_data_complete() {
        let s = [
            ShredStatus::DataComplete,
            ShredStatus::NotDataComplete,
            ShredStatus::NotDataComplete,
            ShredStatus::DataComplete,
        ];
        let tracker = make_test_statustracker(&s);
        assert_eq!(get_indexes(&tracker, 1), Some((1, 3, false)));
    }

    #[test]
    fn start_just_before_data_complete() {
        let s = [
            ShredStatus::DataComplete,
            ShredStatus::NotDataComplete,
            ShredStatus::DataComplete,
        ];
        let tracker = make_test_statustracker(&s);
        assert_eq!(get_indexes(&tracker, 1), Some((1, 2, false)));
    }

    #[test]
    fn two_consecutive_data_complete() {
        let s = [
            ShredStatus::NotDataComplete,
            ShredStatus::DataComplete,
            ShredStatus::DataComplete,
        ];
        let tracker = make_test_statustracker(&s);
        assert_eq!(get_indexes(&tracker, 1), Some((0, 1, false)));
        assert_eq!(get_indexes(&tracker, 2), Some((2, 2, false)));
    }

    #[test]
    fn three_consecutive_data_complete() {
        let s = [
            ShredStatus::NotDataComplete,
            ShredStatus::DataComplete,
            ShredStatus::DataComplete,
            ShredStatus::DataComplete,
            ShredStatus::NotDataComplete,
        ];
        let tracker = make_test_statustracker(&s);
        assert_eq!(get_indexes(&tracker, 1), Some((0, 1, false)));
        assert_eq!(get_indexes(&tracker, 2), Some((2, 2, false)));
        assert_eq!(get_indexes(&tracker, 3), Some((3, 3, false)));
    }

    #[test]
    fn unknown_discards_segment() {
        let s = [
            ShredStatus::NotDataComplete,
            ShredStatus::Unknown,
            ShredStatus::DataComplete,
        ];
        let tracker = make_test_statustracker(&s);
        assert_eq!(get_indexes(&tracker, 0), None);

        let s = [
            ShredStatus::Unknown,
            ShredStatus::NotDataComplete,
            ShredStatus::DataComplete,
        ];
        let tracker = make_test_statustracker(&s);
        assert_eq!(get_indexes(&tracker, 1), Some((1, 2, true)));
    }

    #[test]
    fn test_unknown() {
        let s = [
            ShredStatus::Unknown,
            ShredStatus::DataComplete,
            ShredStatus::DataComplete,
            ShredStatus::NotDataComplete,
            ShredStatus::DataComplete,
        ];
        let tracker = make_test_statustracker(&s);
        assert_eq!(get_indexes(&tracker, 0), None);
        assert_eq!(get_indexes(&tracker, 1), Some((1, 1, true)));
        assert_eq!(get_indexes(&tracker, 2), Some((2, 2, false)));
        assert_eq!(get_indexes(&tracker, 3), Some((3, 4, false)));
    }
}
