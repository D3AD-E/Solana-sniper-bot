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
    /// Highest index written since the last reset.
    ///
    /// The vectors are `MAX_DATA_SHREDS_PER_SLOT` (32768) entries wide because that is the
    /// worst case, but a real slot uses a few thousand. Recycling only has to clear what was
    /// actually touched, and this is how much that is.
    high_water: usize,
}
impl Default for ShredsStateTracker {
    fn default() -> Self {
        Self {
            data_status: vec![ShredStatus::Unknown; MAX_DATA_SHREDS_PER_SLOT],
            data_shreds: vec![None; MAX_DATA_SHREDS_PER_SLOT],
            already_recovered_fec_sets: vec![false; MAX_DATA_SHREDS_PER_SLOT],
            already_deshredded: vec![false; MAX_DATA_SHREDS_PER_SLOT],
            high_water: 0,
        }
    }
}

impl ShredsStateTracker {
    /// Records that `index` has been written, so a later reset knows how far to clear.
    #[inline(always)]
    fn touch(&mut self, index: usize) {
        if index > self.high_water {
            self.high_water = index;
        }
    }

    /// Returns the tracker to its just-constructed state without giving the memory back.
    fn reset(&mut self) {
        let n = (self.high_water + 1).min(self.data_status.len());
        self.data_status[..n].fill(ShredStatus::Unknown);
        self.data_shreds[..n].fill(None);
        self.already_recovered_fec_sets[..n].fill(false);
        self.already_deshredded[..n].fill(false);
        self.high_water = 0;
    }
}

/// Recycles per-slot trackers.
///
/// `Option<Shred>` is 128 bytes, so `data_shreds` alone is 4MiB. Building a tracker per slot
/// means a fresh mapping plus a 4MiB zeroing roughly every 400ms, and the thread that pays
/// it is the one that deshreds and detects -- a launch in the first FEC set of a slot pays it
/// directly. Recycling keeps the pages resident and clears only the touched prefix.
#[derive(Default)]
pub struct TrackerPool {
    free: Vec<ShredsStateTracker>,
}

/// Enough to cover the slots in flight without holding hundreds of megabytes idle.
const MAX_POOLED_TRACKERS: usize = 8;

impl TrackerPool {
    fn take(&mut self) -> ShredsStateTracker {
        self.free.pop().unwrap_or_default()
    }

    fn give(&mut self, mut tracker: ShredsStateTracker) {
        if self.free.len() < MAX_POOLED_TRACKERS {
            tracker.reset();
            self.free.push(tracker);
        }
    }
}

/// `fec_set_index` straight out of the common header, with no parse.
///
/// Layout: signature(64) shred_variant(1) slot(8) index(4) version(2) fec_set_index(4),
/// which is the 83 byte common header. `fec_set_index_matches_the_parsed_shred` checks this
/// against `Shred::fec_set_index` on real captured shreds.
const FEC_SET_INDEX_OFFSET: usize = 64 + 1 + 8 + 4 + 2;

#[inline(always)]
fn peek_fec_set_index(shred: &[u8]) -> Option<u32> {
    let b = shred.get(FEC_SET_INDEX_OFFSET..FEC_SET_INDEX_OFFSET + 4)?;
    Some(u32::from_le_bytes(b.try_into().ok()?))
}

/// Returns the number of shreds reconstructed
/// Updates all_shreds with current state, and deshredded_entries with returned values
/// receive shreds per FEC set, attempting to recover the other shreds in the fec set so you do not have to wait until all data shreds have arrived.
/// every time a fec is recovered, scan for neighbouring DATA_COMPLETE_SHRED flags in the shreds, attempting to deserialize into solana entries when there are no missing shreds between the DATA_COMPLETE_SHRED flags.
/// note that an FEC set doesn't necessarily contain DATA_COMPLETE_SHRED in the last shred. when deserializing the bincode data, you must use data between shreds starting at the last DATA_COMPLETE_SHRED (not inclusive) to the next DATA_COMPLETE_SHRED (inclusive)
pub fn reconstruct_shreds(
    packet_batch: &PacketBatch,
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
    tracker_pool: &mut TrackerPool,
    early: &mut EarlyDetect,
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
        // Reject off the wire bytes, before the shred is copied onto the heap and parsed.
        //
        // `new_from_serialized_shred` allocates and copies ~1228 bytes and then validates
        // the merkle variant, and with several regions subscribed most of what arrives is a
        // shred we already hold. These three fields sit at fixed offsets in the common
        // header, so the duplicate can be dropped for the price of reading them.
        if let (Some(slot), Some(index), Some(fec_set_index)) = (
            solana_ledger::shred::layout::get_slot(packet),
            solana_ledger::shred::layout::get_index(packet),
            peek_fec_set_index(packet),
        ) {
            if highest_slot_seen.saturating_sub(SLOT_LOOKBACK) > slot {
                continue;
            }
            let index = index as usize;
            let fec_set_index = fec_set_index as usize;
            if index < MAX_DATA_SHREDS_PER_SLOT && fec_set_index < MAX_DATA_SHREDS_PER_SLOT {
                if let Some((_, tracker)) = all_shreds.get(&slot) {
                    if tracker.already_recovered_fec_sets[fec_set_index]
                        || tracker.already_deshredded[index]
                    {
                        continue;
                    }
                    // a data shred already held: `update_state_tracker` would reject it too
                    if tracker.data_shreds[index].is_some()
                        && matches!(
                            solana_ledger::shred::layout::get_shred_type(packet),
                            Ok(ShredType::Data)
                        )
                    {
                        continue;
                    }
                }
            }
        }

        match solana_ledger::shred::Shred::new_from_serialized_shred(packet.to_vec())
            .and_then(Shred::try_from)
        {
            Ok(shred) => {
                let slot = shred.common_header().slot;
                let index = shred.index() as usize;
                let fec_set_index = shred.fec_set_index();
                // checked before the parse too, but a shred can arrive for a slot that the
                // batch itself has only just made stale
                if highest_slot_seen.saturating_sub(SLOT_LOOKBACK) > slot {
                    debug!(
                        "Old shred slot: {slot}, fec_set_index: {fec_set_index}, index: {index}"
                    );
                    continue;
                }
                let (all_shreds, state_tracker) = all_shreds
                    .entry(slot)
                    .or_insert_with(|| (Default::default(), tracker_pool.take()));
                if state_tracker.already_recovered_fec_sets[fec_set_index as usize]
                    || state_tracker.already_deshredded[index]
                {
                    debug!("Already completed slot: {slot}, fec_set_index: {fec_set_index}, index: {index}");
                    continue;
                }
                let Some(_shred_index) = update_state_tracker(&shred, state_tracker) else {
                    continue;
                };

                let is_data = matches!(shred.shred_type(), ShredType::Data);
                all_shreds
                    .entry(fec_set_index)
                    .or_default()
                    .insert(ComparableShred(shred));
                if pump_only && is_data {
                    early.note_arrival(slot, index as u32);
                }
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

    // Before FEC recovery and before the ordinary deshred, because this is the whole point:
    // a create is handed over on the shred that completes its transaction, not on the shred
    // that completes the segment around it.
    if pump_only {
        let found = detect_in_partial_segments(all_shreds, early, deshredded_entries);
        if found > 0 {
            metrics
                .early_creates_count
                .fetch_add(found as u64, Ordering::Relaxed);
            metrics.txn_count.fetch_add(found as u64, Ordering::Relaxed);
        }
    }

    // try recovering by FEC set
    // already checked if FEC set is completed or deserialized
    let mut total_recovered_count = 0;
    for (slot, fec_set_index) in slot_fec_indexes_to_iterate.iter() {
        let (all_shreds, state_tracker) = all_shreds
            .entry(*slot)
            .or_insert_with(|| (Default::default(), tracker_pool.take()));
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
                    // a recovered data shred can be the one that completes a create, and on a
                    // lossy feed it often is, so the partial-segment pass has to see it too
                    if pump_only && matches!(shred.shred_type(), ShredType::Data) {
                        early.note_arrival(*slot, shred.index());
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
            state_tracker.touch(*fec_set_index as usize);
            shreds.clear();
        }
    }

    // Again, now that recovery has filled in the gaps. A segment can be contiguous without
    // yet having the `DATA_COMPLETE_SHRED` on its right that the ordinary path below needs,
    // and this pass does not need one.
    if pump_only {
        let found = detect_in_partial_segments(all_shreds, early, deshredded_entries);
        if found > 0 {
            metrics
                .early_creates_count
                .fetch_add(found as u64, Ordering::Relaxed);
            metrics.txn_count.fetch_add(found as u64, Ordering::Relaxed);
        }
    }

    // deshred and bincode deserialize
    for (slot, fec_set_index) in slot_fec_indexes_to_iterate.iter() {
        let (_all_shreds, state_tracker) = all_shreds
            .entry(*slot)
            .or_insert_with(|| (Default::default(), tracker_pool.take()));
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

        // One scan for both discriminators, reused by the walk below. Searching the payload
        // to decide whether to walk it and then searching it again to find out where is two
        // passes over the same tens of kilobytes, and the second pass is on the critical path
        // of the launch we just found.
        let hits = if pump_only {
            create_offsets(&deshredded_payload)
        } else {
            Vec::new()
        };

        // Skip the deserialize for segments that cannot hold a launch. Only done when the
        // segment boundaries are known: when `unknown_start` is set, a deserialize failure is
        // how a mis-bounded segment is detected and left for a later retry, so that signal
        // has to be preserved.
        if pump_only && !unknown_start && hits.is_empty() {
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
                // a direct field write rather than `touch`: the closure captures this field
                // on its own, and `to_deshred` is still borrowing `data_shreds`
                let high = shred.fec_set_index().max(shred.index()) as usize;
                if high > state_tracker.high_water {
                    state_tracker.high_water = high;
                }
            });
            continue;
        }

        // The segment holds a create. Walk it and keep only those transactions rather than
        // materialising every transaction in the segment.
        if pump_only {
            if let Ok(txs) = creates_in_payload(&deshredded_payload, &hits) {
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
                    // a direct field write rather than `touch`: the closure captures this field
                    // on its own, and `to_deshred` is still borrowing `data_shreds`
                    let high = shred.fec_set_index().max(shred.index()) as usize;
                    if high > state_tracker.high_water {
                        state_tracker.high_water = high;
                    }
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
            // a direct field write rather than `touch`: the closure captures this field
            // on its own, and `to_deshred` is still borrowing `data_shreds`
            let high = shred.fec_set_index().max(shred.index()) as usize;
            if high > state_tracker.high_water {
                state_tracker.high_water = high;
            }
        })
    }

    if all_shreds.len() > MAX_PROCESSING_AGE {
        let slot_threshold = highest_slot_seen.saturating_sub(SLOT_LOOKBACK);
        let mut incomplete_fec_sets = ahash::HashMap::<Slot, Vec<_>>::default();
        let mut incomplete_fec_sets_count = 0;
        // `retain` can only drop the tracker; taking the entries out lets the 4MiB of
        // vectors go back to the pool and be reused by the next slot
        let stale = all_shreds
            .keys()
            .copied()
            .filter(|slot| *slot < slot_threshold)
            .collect::<Vec<Slot>>();
        for slot in stale {
            let Some((fec_set_indexes, state_tracker)) = all_shreds.remove(&slot) else {
                continue;
            };

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
                    .entry(slot)
                    .and_modify(|fec_set_data| {
                        fec_set_data.push((*fec_set_index, num_expected_data_shreds, shreds.len()))
                    })
                    .or_insert_with(|| {
                        vec![(*fec_set_index, num_expected_data_shreds, shreds.len())]
                    });
            }

            tracker_pool.give(state_tracker);
        }
        early.segments.retain(|(slot, _), _| *slot >= slot_threshold);
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
        state_tracker.touch(index);
    };
    Some(index)
}


/// True when the deshredded payload might contain a pump.fun create.
///
/// Superseded on the hot path by `create_offsets`, which answers the same question and also
/// says where; kept as the reference the fused scan is tested against.
///
/// The payload here is already assembled and contiguous, so an instruction's 8 byte
/// discriminator cannot be split the way a pubkey could be split across raw shreds — which
/// is what made the old shred-level prefilter drop launches. Deserializing a segment costs
/// ~54us and only about one segment in six hundred holds a create, so this scan pays for
/// itself many times over.
#[cfg_attr(not(test), allow(dead_code))]
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


/// Returns the transactions in this segment that are pump.fun creates.
///
/// Decoding the whole segment costs ~43us because every transaction allocates. Instead the
/// discriminator offsets are located with a substring search, the entry vector is *walked*
/// by reading lengths, and only a transaction whose byte span contains a hit is handed to
/// bincode. Walking stops past the last hit, so the tail is never touched.
///
/// Returns `Err(())` when the payload does not walk cleanly, and the caller falls back to
/// the ordinary deserialize so a malformed segment behaves exactly as before.
fn creates_in_payload(
    payload: &[u8],
    hits: &[usize],
) -> Result<Vec<solana_sdk::transaction::VersionedTransaction>, ()> {
    if hits.is_empty() {
        return Ok(Vec::new());
    }
    let candidates = sniper::wire::transactions_at(payload, hits).ok_or(())?;
    Ok(candidates
        .into_iter()
        .filter(|tx| sniper::pumpfun::parse_create(tx).is_some())
        .collect())
}

/// Length of an anchor instruction discriminator.
const DISC_LEN: usize = 8;

/// Offsets of every create discriminator in the payload, in order.
#[inline]
fn create_offsets(payload: &[u8]) -> Vec<usize> {
    let mut hits = Vec::new();
    create_offsets_into(payload, 0, &mut hits);
    hits
}

/// Appends the offsets of every create discriminator in `slice`, shifted by `base`.
///
/// The base exists for the incremental scan, which searches only the bytes that have just
/// arrived but has to report offsets into the whole segment.
#[inline]
fn create_offsets_into(slice: &[u8], base: usize, hits: &mut Vec<usize>) {
    hits.extend(memchr::memmem::find_iter(slice, &sniper::pumpfun::DISC_CREATE_V2).map(|o| o + base));
    hits.extend(memchr::memmem::find_iter(slice, &sniper::pumpfun::DISC_CREATE).map(|o| o + base));
    hits.sort_unstable();
}



// ---------------------------------------------------------------------------
// early detection
// ---------------------------------------------------------------------------

/// Incremental detection over the part of a segment that has arrived.
///
/// The ordinary path cannot produce a transaction until the segment it lives in is bounded
/// on both sides by `DATA_COMPLETE_SHRED` flags, because that is what `Shredder::deshred`
/// requires. A 32 shred FEC set arrives over a millisecond or more, so a create sitting in
/// the third shred is invisible for the rest of that millisecond -- next to which the whole
/// internal detect path, at ~26us, is noise.
///
/// Nothing actually forces the wait. A segment is a bincode `Vec<Entry>` written in order, so
/// the prefix that has arrived is a valid prefix of that encoding, and `wire::transactions_at`
/// already refuses to guess when it walks off the end. So each segment keeps the data it has
/// seen so far, appends newly arrived shreds to it, searches only the newly appended bytes,
/// and hands over any create whose transaction is complete. The create fires on the shred
/// that finishes it rather than on the shred that finishes the segment.
///
/// The ordinary path still runs and still produces the full segment for the entry feed. A
/// launch found twice is not a problem: the sniper dedups on mint.
#[derive(Default)]
pub struct EarlyDetect {
    /// keyed by (slot, first data shred index of the segment)
    segments: ahash::HashMap<(Slot, u32), EarlySegment>,
    /// data shred indexes ingested in the current batch
    arrived: Vec<(Slot, u32)>,
    /// scratch, reused across segments
    hits: Vec<usize>,
}

impl EarlyDetect {
    /// Records a data shred that just arrived, so the segment holding it is grown and
    /// rescanned. Keyed on the shred index rather than the FEC set index: a segment is
    /// bounded by `DATA_COMPLETE_SHRED` flags, and several of them fit inside one FEC set.
    #[inline]
    pub fn note_arrival(&mut self, slot: Slot, index: u32) {
        self.arrived.push((slot, index));
    }
}

struct EarlySegment {
    /// next data shred index to append
    next: u32,
    /// concatenated data payloads of shreds `[start, next)`
    buf: Vec<u8>,
    /// how much of `buf` has already been searched for a create discriminator
    scanned: usize,
    /// discriminator offsets found so far, including ones whose transaction has not
    /// finished arriving yet
    hits: Vec<usize>,
    /// the segment's last shred has arrived; the ordinary path owns it from here
    complete: bool,
}

impl EarlySegment {
    fn new(start: u32) -> Self {
        Self {
            next: start,
            buf: Vec::with_capacity(8 * 1024),
            scanned: 0,
            hits: Vec::new(),
            complete: false,
        }
    }
}

/// First data shred index of the segment that `index` belongs to.
///
/// Same left boundary the ordinary path uses -- the shred after the previous
/// `DATA_COMPLETE_SHRED` -- but this one refuses to guess. `get_indexes` is allowed to pick a
/// best-effort start because a wrong guess shows up as a bincode failure it can retry; here a
/// wrong start would just waste the scan, so an unknown boundary means "not yet".
fn segment_start(tracker: &ShredsStateTracker, index: usize) -> Option<u32> {
    if index >= tracker.data_status.len() || tracker.already_deshredded[index] {
        return None;
    }
    let mut start = index;
    while start > 0 {
        match tracker.data_status[start - 1] {
            // the previous shred ends the previous segment, so this one starts here
            ShredStatus::DataComplete => return Some(start as u32),
            ShredStatus::NotDataComplete => {
                if tracker.already_deshredded[start - 1] {
                    return Some(start as u32);
                }
                start -= 1;
            }
            ShredStatus::Unknown => return None,
        }
    }
    Some(0)
}

/// Grows every segment that gained a shred and hands over any create that is now complete.
/// Returns the number of launches found.
fn detect_in_partial_segments(
    all_shreds: &ahash::HashMap<
        Slot,
        (
            ahash::HashMap<u32, HashSet<ComparableShred>>,
            ShredsStateTracker,
        ),
    >,
    early: &mut EarlyDetect,
    deshredded_entries: &mut Vec<(Slot, Vec<solana_entry::entry::Entry>, Vec<u8>)>,
) -> usize {
    // taken out so the segment map can be mutated while this is iterated; put back empty
    let mut arrived = std::mem::take(&mut early.arrived);
    arrived.sort_unstable();
    arrived.dedup();

    let mut found = 0usize;
    for (slot, shred_index) in arrived.iter() {
        let Some((_, tracker)) = all_shreds.get(slot) else {
            continue;
        };
        let Some(start) = segment_start(tracker, *shred_index as usize) else {
            continue;
        };

        let seg = early
            .segments
            .entry((*slot, start))
            .or_insert_with(|| EarlySegment::new(start));
        if seg.complete {
            continue;
        }

        // append every contiguous data shred that has turned up since last time
        let mut ends_here = false;
        while (seg.next as usize) < tracker.data_status.len() {
            let i = seg.next as usize;
            if tracker.already_deshredded[i] {
                // the ordinary path got there first
                ends_here = true;
                break;
            }
            let Some(shred) = tracker.data_shreds[i].as_ref() else {
                break;
            };
            let Ok(data) = solana_ledger::shred::layout::get_data(shred.payload().as_ref())
            else {
                break;
            };
            seg.buf.extend_from_slice(data);
            seg.next += 1;
            if matches!(tracker.data_status[i], ShredStatus::DataComplete) {
                ends_here = true;
                break;
            }
        }

        // Scan before retiring the segment. When the shred that finishes a transaction is
        // also the one that finishes the segment there is nothing to gain, but the ordinary
        // path only picks the segment up if it can bound it on the left too -- and this path
        // does not care about the left boundary of the *previous* segment.
        if seg.buf.len() > seg.scanned {
            let from = seg.scanned.saturating_sub(DISC_LEN - 1);
            early.hits.clear();
            create_offsets_into(&seg.buf[from..], from, &mut early.hits);
            seg.scanned = seg.buf.len();
            if !early.hits.is_empty() {
                seg.hits.extend_from_slice(&early.hits);
                seg.hits.sort_unstable();
                seg.hits.dedup();
            }

            // `Err` means the walk ran off the end: the create's transaction has not finished
            // arriving, so the hits stay recorded and the next shred tries again
            if !seg.hits.is_empty() {
                if let Ok(txs) = creates_in_payload(&seg.buf, &seg.hits) {
                    if !txs.is_empty() {
                        found += txs.len();
                        // an empty payload marks a partial segment: it must not reach the
                        // entry feed, which promises whole segments
                        deshredded_entries.push((
                            *slot,
                            vec![solana_entry::entry::Entry {
                                num_hashes: 0,
                                hash: solana_sdk::hash::Hash::default(),
                                transactions: txs,
                            }],
                            Vec::new(),
                        ));
                    }
                }
            }
        }

        if ends_here {
            seg.complete = true;
            // the ordinary path owns it now, and it will publish the whole segment
            seg.buf = Vec::new();
            seg.hits = Vec::new();
        }
    }

    arrived.clear();
    early.arrived = arrived;
    found
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
        deshred::{reconstruct_shreds, ComparableShred, EarlyDetect, TrackerPool},
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
            &PacketBatch::new(
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
            &mut TrackerPool::default(),
            &mut EarlyDetect::default(),
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
            &PacketBatch::new(
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
            &mut TrackerPool::default(),
            &mut EarlyDetect::default(),
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
            &PacketBatch::new(
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
            &mut TrackerPool::default(),
            &mut EarlyDetect::default(),
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
            &PacketBatch::new(
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
            &mut TrackerPool::default(),
            &mut EarlyDetect::default(),
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
            &PacketBatch::new(packets.clone()),
            &mut all_shreds,
            &mut slot_fec_indexes_to_iterate,
            &mut deshredded_entries,
            &mut highest_slot_seen,
            &mut TrackerPool::default(),
            &mut EarlyDetect::default(),
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
            &PacketBatch::new(
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
            &mut TrackerPool::default(),
            &mut EarlyDetect::default(),
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
            &batch,
            &mut all_shreds,
            &mut idx,
            &mut out,
            &mut highest,
            &mut TrackerPool::default(),
            &mut EarlyDetect::default(),
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

        // 4b. the streaming walk, on the segment that actually holds a create
        let with_create: Vec<&Vec<u8>> = payloads
            .iter()
            .filter(|p| super::may_contain_pump_create(p))
            .collect();
        if !with_create.is_empty() {
            let t = std::time::Instant::now();
            let reps = 200;
            for _ in 0..reps {
                for p in &with_create {
                    let hits = super::create_offsets(p);
                    std::hint::black_box(super::creates_in_payload(p, &hits).ok());
                }
            }
            println!(
                "streaming walk:         {:>8}us per create segment (vs ~44us full deserialize)",
                t.elapsed().as_micros() as f64 / (reps * with_create.len()) as f64
            );
        }

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
            &PacketBatch::new(
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
            &mut TrackerPool::default(),
            &mut EarlyDetect::default(),
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

            // the fused scan must agree with the standalone one it replaced
            let hits = super::create_offsets(payload);
            assert_eq!(
                !hits.is_empty(),
                super::may_contain_pump_create(payload),
                "create_offsets and may_contain_pump_create disagreed"
            );
            let fast: Vec<_> = if !hits.is_empty() {
                super::creates_in_payload(payload, &hits)
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

    /// The pre-parse filter reads `fec_set_index` straight out of the common header, at a
    /// hardcoded offset. If that offset is ever wrong the filter starts dropping live shreds
    /// while everything still compiles, so it is checked against the parsed value on every
    /// shred in the capture.
    #[test]
    fn peeked_fec_set_index_matches_the_parsed_shred() {
        let Ok(buffer) = std::fs::read("../bins/serialized_shreds.bin") else {
            eprintln!("skipping: ../bins/serialized_shreds.bin not present (git lfs)");
            return;
        };
        let packets = Packets::try_from_slice(&buffer).unwrap();

        let mut checked = 0usize;
        for raw in packets.packets.iter() {
            let Ok(parsed) =
                solana_ledger::shred::Shred::new_from_serialized_shred(raw.clone())
            else {
                continue;
            };
            assert_eq!(
                super::peek_fec_set_index(raw),
                Some(parsed.fec_set_index()),
                "peeked fec_set_index disagreed with the parsed shred"
            );
            assert_eq!(
                solana_ledger::shred::layout::get_slot(raw),
                Some(parsed.slot())
            );
            assert_eq!(
                solana_ledger::shred::layout::get_index(raw),
                Some(parsed.index())
            );
            checked += 1;
        }
        assert!(checked > 1000, "only {checked} shreds parsed out of the capture");
    }


    /// Early detection has to be both *earlier* and *right*.
    ///
    /// The capture is replayed one shred per batch, which is how they actually arrive. For
    /// every create, the batch that first reported it through a partial segment is compared
    /// against the batch that reported it through the ordinary complete-segment path. The
    /// partial path must never invent a launch, and for real launches it must get there first
    /// at least some of the time -- that lead is the whole reason it exists.

    /// Builds a pump.fun `create_v2` transaction that `parse_create` accepts.
    fn synthetic_create_tx(
        creator: solana_sdk::pubkey::Pubkey,
    ) -> solana_sdk::transaction::VersionedTransaction {
        use solana_sdk::instruction::{AccountMeta, Instruction};

        // accounts 0 mint, 1 mint_authority, 2 bonding_curve, 3 associated_bonding_curve,
        // 4 global, 5 user, 6 system, 7 token_2022
        let accounts = (0..8)
            .map(|i| {
                if i == 7 {
                    AccountMeta::new_readonly(sniper::pumpfun::TOKEN_2022_PROGRAM, false)
                } else {
                    AccountMeta::new(solana_sdk::pubkey::Pubkey::new_unique(), false)
                }
            })
            .collect::<Vec<_>>();

        // disc | name/symbol/uri | creator | is_mayhem_mode | is_cashback_enabled
        let mut data = sniper::pumpfun::DISC_CREATE_V2.to_vec();
        data.extend_from_slice(&[0xEEu8; 48]);
        data.extend_from_slice(&creator.to_bytes());
        data.extend_from_slice(&[0u8, 0u8]);

        let ix = Instruction {
            program_id: sniper::pumpfun::PUMP_PROGRAM,
            accounts,
            data,
        };
        let payer = solana_sdk::pubkey::Pubkey::new_unique();
        solana_sdk::transaction::VersionedTransaction {
            signatures: vec![solana_sdk::signature::Signature::default()],
            message: solana_sdk::message::VersionedMessage::Legacy(
                solana_sdk::message::Message::new(&[ix], Some(&payer)),
            ),
        }
    }

    /// A transaction that carries `bytes` of payload and nothing that looks like a create.
    fn filler_tx(bytes: usize) -> solana_sdk::transaction::VersionedTransaction {
        use solana_sdk::instruction::{AccountMeta, Instruction};
        let ix = Instruction {
            program_id: solana_sdk::pubkey::Pubkey::new_unique(),
            accounts: vec![AccountMeta::new(
                solana_sdk::pubkey::Pubkey::new_unique(),
                false,
            )],
            data: vec![0xEEu8; bytes],
        };
        let payer = solana_sdk::pubkey::Pubkey::new_unique();
        solana_sdk::transaction::VersionedTransaction {
            signatures: vec![solana_sdk::signature::Signature::default()],
            message: solana_sdk::message::VersionedMessage::Legacy(
                solana_sdk::message::Message::new(&[ix], Some(&payer)),
            ),
        }
    }

    /// The point of the whole partial-segment path, stated as a test.
    ///
    /// A segment holding a create near its front is shredded and delivered one shred at a
    /// time, in order, the way a healthy feed delivers them. The ordinary path cannot say
    /// anything until the last shred arrives, because that is the one carrying
    /// `DATA_COMPLETE_SHRED`. The partial path has to produce the launch as soon as the
    /// transaction itself is complete, which is many shreds earlier -- and that gap is the
    /// entire reason to run a sniper off shreds rather than off a block feed.
    #[test]
    fn a_create_is_found_before_its_segment_ends() {
        let slot = 42_424u64;
        let leader_keypair = Arc::new(Keypair::new());
        let reed_solomon_cache = ReedSolomonCache::default();
        let shredder = Shredder::new(slot, slot - 1, 0, 0).unwrap();

        let creator = solana_sdk::pubkey::Pubkey::new_unique();
        let create = synthetic_create_tx(creator);
        let expected_creator = sniper::pumpfun::parse_create(&create)
            .expect("the synthetic transaction must parse as a create")
            .creator;

        // the create up front, then enough traffic behind it to span a lot of shreds.
        // Entries are built through `make_slot_entries_with_transactions` because the
        // shredder wants the `Entry` from the vendored ledger, not the one from crates.io.
        let mut entries = make_slot_entries_with_transactions(1);
        entries[0].transactions = vec![create];
        for _ in 0..30 {
            let mut filler = make_slot_entries_with_transactions(1);
            filler[0].transactions = (0..2).map(|_| filler_tx(900)).collect();
            entries.extend(filler);
        }

        let (data_shreds, _coding) = shredder.entries_to_shreds(
            &leader_keypair,
            entries.as_slice(),
            true, // is_last_in_slot: DATA_COMPLETE lands on the final data shred
            Some(Hash::new_from_array(rand::thread_rng().gen())),
            0,
            0,
            true,
            &reed_solomon_cache,
            &mut ProcessShredsStats::default(),
        );
        assert!(
            data_shreds.len() > 10,
            "need a segment worth several shreds, got {}",
            data_shreds.len()
        );

        let rs_cache = ReedSolomonCache::default();
        let metrics = Arc::new(ShredMetrics::default());
        let mut all_shreds = ahash::HashMap::default();
        let mut slot_fec_indexes_to_iterate: Vec<(Slot, u32)> = Vec::new();
        let mut deshredded_entries = Vec::new();
        let mut highest_slot_seen = 0;
        let mut tracker_pool = TrackerPool::default();
        let mut early = EarlyDetect::default();

        let mut early_at: Option<usize> = None;
        let mut full_at: Option<usize> = None;

        // data shreds only, in order, one per call: no coding shreds, so nothing can be
        // recovered and every shred has to earn its place
        for (i, shred) in data_shreds.iter().enumerate() {
            let mut packet = Packet::default();
            shred.copy_to_packet(&mut packet);

            reconstruct_shreds(
                &PacketBatch::new(vec![packet]),
                &mut all_shreds,
                &mut slot_fec_indexes_to_iterate,
                &mut deshredded_entries,
                &mut highest_slot_seen,
                &mut tracker_pool,
                &mut early,
                &rs_cache,
                &metrics,
                true,
            );

            for (_slot, decoded, payload) in deshredded_entries.iter() {
                let hit = decoded
                    .iter()
                    .flat_map(|e| e.transactions.iter())
                    .filter_map(sniper::pumpfun::parse_create)
                    .any(|info| info.creator == expected_creator);
                if !hit {
                    continue;
                }
                // an empty payload marks a partial segment
                let slot_at = if payload.is_empty() {
                    &mut early_at
                } else {
                    &mut full_at
                };
                slot_at.get_or_insert(i);
            }
        }

        let early_at = early_at.expect("the create was never found in a partial segment");
        let full_at = full_at.expect("the create was never found in the complete segment");
        println!(
            "segment of {} shreds: partial path found the create at shred {early_at}, \
             complete path at shred {full_at}",
            data_shreds.len()
        );
        assert_eq!(
            full_at,
            data_shreds.len() - 1,
            "the complete path can only speak once DATA_COMPLETE has arrived"
        );
        assert!(
            early_at < full_at,
            "partial path bought nothing: {early_at} vs {full_at}"
        );
    }

    #[test]
    fn partial_segments_find_creates_before_the_segment_completes() {
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
        let mut tracker_pool = TrackerPool::default();
        let mut early = EarlyDetect::default();

        // mint -> index of the batch that first reported it, per path
        let mut first_early: std::collections::HashMap<solana_sdk::pubkey::Pubkey, usize> =
            Default::default();
        let mut first_full: std::collections::HashMap<solana_sdk::pubkey::Pubkey, usize> =
            Default::default();

        for (batch_index, raw) in packets.packets.iter().enumerate() {
            let mut packet = Packet::default();
            packet.buffer_mut()[..raw.len()].copy_from_slice(raw);
            packet.meta_mut().size = raw.len();

            reconstruct_shreds(
                &PacketBatch::new(vec![packet]),
                &mut all_shreds,
                &mut slot_fec_indexes_to_iterate,
                &mut deshredded_entries,
                &mut highest_slot_seen,
                &mut tracker_pool,
                &mut early,
                &rs_cache,
                &metrics,
                true,
            );

            for (_slot, entries, payload) in deshredded_entries.iter() {
                // an empty payload is how a partial segment is marked
                let target = if payload.is_empty() {
                    &mut first_early
                } else {
                    &mut first_full
                };
                for tx in entries.iter().flat_map(|e| e.transactions.iter()) {
                    if let Some(info) = sniper::pumpfun::parse_create(tx) {
                        target.entry(info.mint).or_insert(batch_index);
                    }
                }
            }
        }

        assert!(
            !first_full.is_empty(),
            "the ordinary path should still find creates"
        );
        assert!(
            !first_early.is_empty(),
            "no create was found before its segment completed"
        );

        // never invent a launch
        for mint in first_early.keys() {
            assert!(
                first_full.contains_key(mint),
                "partial segment reported {mint}, which the complete segment never produced"
            );
        }

        let mut earlier = 0usize;
        let mut total_lead = 0usize;
        for (mint, early_at) in first_early.iter() {
            let full_at = first_full[mint];
            assert!(
                *early_at <= full_at,
                "{mint} was reported late by the partial path: {early_at} vs {full_at}"
            );
            if *early_at < full_at {
                earlier += 1;
                total_lead += full_at - *early_at;
            }
        }

        println!(
            "early detection: {} of {} creates found before their segment completed, \
             {} of them strictly earlier, {:.1} shreds of lead on average",
            first_early.len(),
            first_full.len(),
            earlier,
            total_lead as f64 / earlier.max(1) as f64,
        );
        // No lead is asserted here. This capture is lossy enough that its one create only
        // becomes available through FEC recovery, which hands the whole segment over at once
        // -- so on this input both paths land on the same shred. What is asserted is the part
        // that has to hold on every input: the partial path never invents a launch and is
        // never the slower of the two. `a_create_is_found_before_its_segment_ends` covers the
        // lead itself, deterministically.
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
            &PacketBatch::new(
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
            &mut TrackerPool::default(),
            &mut EarlyDetect::default(),
            &rs_cache,
            &metrics,
            true,
        );

        let mut txn_count = 0usize;
        let mut creates = 0usize;
        let mut early_creates = 0usize;
        let mut mints = HashSet::new();
        for (_slot, entries, payload) in &deshredded_entries {
            // an empty payload marks a partial segment, which reports a create as soon as its
            // transaction is complete. The complete segment reports it again afterwards, and
            // the sniper dedups on mint -- so only the complete-segment path is counted here
            let partial = payload.is_empty();
            for entry in entries {
                txn_count += entry.transactions.len();
                for tx in &entry.transactions {
                    if let Some(info) = sniper::pumpfun::parse_create(tx) {
                        mints.insert(info.mint);
                        if partial {
                            early_creates += 1;
                        } else {
                            creates += 1;
                        }
                    }
                }
            }
        }

        let slots = deshredded_entries
            .iter()
            .map(|(slot, _, _)| *slot)
            .collect::<HashSet<_>>();
        println!(
            "replay: {} segments over {} slots, {txn_count} transactions, {creates} pump creates              ({early_creates} of them also found before their segment completed), {} distinct mints",
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
        assert_eq!(
            creates,
            mints.len(),
            "each create should be seen once through the complete-segment path"
        );
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
