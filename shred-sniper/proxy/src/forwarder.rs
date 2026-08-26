use std::{
    collections::HashSet,
    net::{IpAddr, Ipv6Addr, SocketAddr, UdpSocket},
    sync::{
        atomic::{AtomicBool, AtomicU64, Ordering},
        Arc, RwLock,
    },
    thread::{Builder, JoinHandle},
    time::{Duration, SystemTime},
};

use arc_swap::ArcSwap;
use crossbeam_channel::{Receiver, RecvError};
use dashmap::DashMap;
use itertools::Itertools;
use jito_protos::shredstream::{
    Entry as PbEntry, Fill as PbFill, PumpCreate as PbPumpCreate, TraceShred,
};
use log::{debug, error, info, warn};
use prost::Message;
use solana_client::client_error::reqwest;
use solana_ledger::shred::ReedSolomonCache;
use solana_metrics::{datapoint_info, datapoint_warn};
use solana_net_utils::SocketConfig;
use solana_perf::{
    deduper::Deduper,
    packet::{PacketBatch, PacketBatchRecycler},
    recycler::Recycler,
};
use solana_sdk::clock::Slot;
use solana_streamer::{
    sendmmsg::{batch_send, SendPktsError},
    streamer::{self, StreamerReceiveStats},
};
use tokio::sync::broadcast::Sender;

use sniper::{pumpfun, HotSniper, Sniper};

use crate::{
    deshred,
    deshred::{ComparableShred, ShredsStateTracker},
    resolve_hostname_port, ShredstreamProxyError,
};

// values copied from https://github.com/solana-labs/solana/blob/33bde55bbdde13003acf45bb6afe6db4ab599ae4/core/src/sigverify_shreds.rs#L20
pub const DEDUPER_FALSE_POSITIVE_RATE: f64 = 0.001;
pub const DEDUPER_NUM_BITS: u64 = 637_534_199; // 76MB
pub const DEDUPER_RESET_CYCLE: Duration = Duration::from_secs(5 * 60);

/// Bind to ports and start forwarding shreds
#[allow(clippy::too_many_arguments)]
pub fn start_forwarder_threads(
    unioned_dest_sockets: Arc<ArcSwap<Vec<SocketAddr>>>, /* sockets shared between endpoint discovery thread and forwarders */
    src_addr: IpAddr,
    src_port: u16,
    maybe_multicast_socket: Option<Vec<UdpSocket>>,
    num_threads: Option<usize>,
    deduper: Arc<RwLock<Deduper<2, [u8]>>>,
    should_reconstruct_shreds: bool,
    entry_sender: Arc<Sender<PbEntry>>,
    pump_create_sender: Arc<Sender<PbPumpCreate>>,
    fill_sender: Arc<Sender<PbFill>>,
    sniper: Option<Sniper>,
    debug_trace_shred: bool,
    use_discovery_service: bool,
    forward_stats: Arc<StreamerReceiveStats>,
    metrics: Arc<ShredMetrics>,
    shutdown_receiver: Receiver<()>,
    exit: Arc<AtomicBool>,
) -> Vec<JoinHandle<()>> {
    let num_threads = num_threads
        .unwrap_or_else(|| usize::from(std::thread::available_parallelism().unwrap()).min(4));

    let recycler: PacketBatchRecycler = Recycler::warmed(100, 1024);

    // multi_bind_in_range returns (port, Vec<UdpSocket>)
    let (_port, sockets) = solana_net_utils::multi_bind_in_range_with_config(
        src_addr,
        (src_port, src_port + 1),
        SocketConfig::default().reuseport(true),
        num_threads,
    )
    .unwrap_or_else(|_| {
        panic!("Failed to bind listener sockets. Check that port {src_port} is not in use.")
    });

    let (reconstruct_tx, reconstruct_rx) =
        crossbeam_channel::bounded::<Arc<PacketBatch>>(1_024);
    let mut thread_hdls = Vec::with_capacity(num_threads + 1);

    if should_reconstruct_shreds {
        let metrics = metrics.clone();
        let exit = exit.clone();
        // receives shreds from recv_from_channel_and_send_multiple_dest and calls deshred::reconstruct_shreds
        let hdl = std::thread::Builder::new()
            .name("shred_reconstructor".to_string())
            .spawn(move || {
                let mut all_shreds = ahash::HashMap::<
                    Slot,
                    (
                        ahash::HashMap<u32, HashSet<ComparableShred>>,
                        ShredsStateTracker,
                    ),
                >::default();
                let mut slot_fec_indexes_to_iterate = Vec::<(Slot, u32)>::new();
                let mut deshredded_entries =
                    Vec::<(Slot, Vec<solana_entry::entry::Entry>, Vec<u8>)>::new();
                let mut highest_slot_seen: Slot = 0;
                // per-slot trackers are 4MiB each; recycling them keeps that allocation and
                // its zeroing off this thread at every slot boundary
                let mut tracker_pool = deshred::TrackerPool::default();
                // per-segment state for firing on a create before its segment is complete
                let mut early = deshred::EarlyDetect::default();
                let rs_cache = ReedSolomonCache::default();
                // hot sniper state lives on this thread only: detection and firing happen
                // inline, before anything is serialized or sent anywhere else
                let mut hot_sniper: Option<HotSniper> = sniper.map(|s| {
                    let (hot, _threads) = s.into_hot();
                    hot
                });

                while !exit.load(Ordering::Relaxed) {
                    match reconstruct_rx.recv_timeout(Duration::from_millis(100)) {
                        Ok(pkt_batch) => {
                            deshred::reconstruct_shreds(
                                &pkt_batch,
                                &mut all_shreds,
                                &mut slot_fec_indexes_to_iterate,
                                &mut deshredded_entries,
                                &mut highest_slot_seen,
                                &mut tracker_pool,
                                &mut early,
                                &rs_cache,
                                &metrics,
                                // only segments that can hold a launch are worth deserializing
                                true,
                            );

                            deshredded_entries.drain(..).for_each(
                                |(slot, entries, entries_bytes)| {
                                    scan_for_pump_creates(
                                        slot,
                                        &entries,
                                        hot_sniper.as_mut(),
                                        &pump_create_sender,
                                        &fill_sender,
                                        &metrics,
                                    );
                                    // an empty payload is an early detection: a partial
                                    // segment, which the entry feed must not be given. The
                                    // ordinary path publishes the whole segment later.
                                    if !entries_bytes.is_empty() {
                                        let _ = entry_sender.send(PbEntry {
                                            slot,
                                            entries: entries_bytes,
                                        });
                                    }
                                },
                            );
                        }
                        Err(crossbeam_channel::RecvTimeoutError::Timeout) => {} // do nothing
                        Err(crossbeam_channel::RecvTimeoutError::Disconnected) => break,
                    }
                }
            })
            .unwrap();
        thread_hdls.push(hdl);
    };

    sockets
        .into_iter()
        .chain(maybe_multicast_socket.into_iter().flatten())
        .enumerate()
        .flat_map(|(thread_id, incoming_shred_socket)| {
            let (packet_sender, packet_receiver) = crossbeam_channel::unbounded();
            let listen_thread = streamer::receiver(
                format!("ssListen{thread_id}"),
                Arc::new(incoming_shred_socket),
                exit.clone(),
                packet_sender,
                recycler.clone(),
                forward_stats.clone(),
                Duration::default(),
                false,
                None,
                false,
            );

            let deduper = deduper.clone();
            let unioned_dest_sockets = unioned_dest_sockets.clone();
            let metrics = metrics.clone();
            let shutdown_receiver = shutdown_receiver.clone();
            let reconstruct_tx = reconstruct_tx.clone();
            let exit = exit.clone();

            let send_thread = Builder::new()
                .name(format!("ssPxyTx_{thread_id}"))
                .spawn(move || {
                    let send_socket =
                        UdpSocket::bind(SocketAddr::new(IpAddr::V6(Ipv6Addr::UNSPECIFIED), 0))
                            .expect("to bind to udp port for forwarding");
                    let mut local_dest_sockets = unioned_dest_sockets.load();

                    let refresh_subscribers_tick = if use_discovery_service {
                        crossbeam_channel::tick(Duration::from_secs(30))
                    } else {
                        crossbeam_channel::tick(Duration::MAX)
                    };

                    while !exit.load(Ordering::Relaxed) {
                        crossbeam_channel::select! {
                            // forward packets
                            recv(packet_receiver) -> maybe_packet_batch => {
                                let res = recv_from_channel_and_send_multiple_dest(
                                    maybe_packet_batch,
                                    &deduper,
                                    &send_socket,
                                    &local_dest_sockets,
                                    should_reconstruct_shreds,
                                    &reconstruct_tx,
                                    debug_trace_shred,
                                    &metrics,
                                );

                                // If the channel is closed or error, break out
                                if res.is_err() {
                                    break;
                                }
                            }

                            // refresh thread-local subscribers
                            recv(refresh_subscribers_tick) -> _ => {
                                local_dest_sockets = unioned_dest_sockets.load();
                            }

                            // handle shutdown (avoid using sleep since it can hang)
                            recv(shutdown_receiver) -> _ => {
                                break;
                            }
                        }
                    }
                    info!("Exiting forwarder thread {thread_id}.");
                })
                .unwrap();

            vec![listen_thread, send_thread]
        })
        .collect::<Vec<JoinHandle<()>>>()
}

/// Maps a fired launch to the Fill message the Node seller consumes.
///
/// The wire contract the seller depends on: `max_sol_cost` carries the
/// instruction-INDEPENDENT cost basis in lamports (`FiredLaunch::cost_lamports`) and
/// `amount` the expected token fill — NOT the raw buy-instruction args, whose meaning flips
/// with `buy_exact_sol_in` (arg1 becomes a token floor there; the seller once priced its
/// stop-loss off it and dumped every position at slot +1).
fn fill_from_fired(slot: Slot, f: &sniper::FiredLaunch) -> PbFill {
    PbFill {
        slot,
        mint: f.mint.to_bytes().to_vec(),
        token_account: f.token_account.to_bytes().to_vec(),
        seed: String::from_utf8_lossy(&f.seed).into_owned(),
        token_program: f.token_program.to_bytes().to_vec(),
        bonding_curve: f.bonding_curve.to_bytes().to_vec(),
        associated_bonding_curve: f.associated_bonding_curve.to_bytes().to_vec(),
        creator: f.creator.to_bytes().to_vec(),
        amount: f.expected_tokens,
        max_sol_cost: f.cost_lamports,
        fired_at_micros: SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_micros() as u64)
            .unwrap_or_default(),
    }
}

/// Runs pump.fun detection on *reconstructed entries*.
///
/// This is deliberately not a prefilter over raw shred payloads: a 32 byte pubkey can
/// straddle two shreds and can appear only inside coding shreds, so byte scanning drops
/// real launches. By the time we get here the transaction is already parsed, so detection
/// costs a discriminator compare per pump instruction.
#[inline]
fn scan_for_pump_creates(
    slot: Slot,
    entries: &[solana_entry::entry::Entry],
    mut hot: Option<&mut HotSniper>,
    pump_create_sender: &Sender<PbPumpCreate>,
    fill_sender: &Sender<PbFill>,
    metrics: &ShredMetrics,
) {
    for entry in entries {
        for tx in &entry.transactions {
            // A launch either fires immediately (whitelist path) or, under the confirmation
            // trigger, fires from a confirming buy in the create block. Both return a
            // FiredLaunch carrying everything the seller needs.
            let create_info = pumpfun::parse_create(tx);
            let fired = if let Some(info) = &create_info {
                metrics.pump_creates_seen.fetch_add(1, Ordering::Relaxed);
                // fire first: everything below allocates or formats
                hot.as_mut().and_then(|h| h.on_create(info, slot))
            } else if let Some(buy) = pumpfun::parse_buy(tx) {
                hot.as_mut().and_then(|h| h.on_buy(&buy, slot))
            } else {
                continue;
            };

            // the seller needs the seed: the buy creates its token account from one, so the
            // address cannot be re-derived from the mint alone
            if let Some(f) = fired {
                metrics.snipes_fired.fetch_add(1, Ordering::Relaxed);
                if fill_sender.receiver_count() > 0 {
                    let _ = fill_sender.send(fill_from_fired(slot, &f));
                }
            }

            // downstream create feed: only for actual creates, not confirming buys
            let Some(info) = &create_info else {
                continue;
            };
            if pump_create_sender.receiver_count() == 0 {
                continue;
            }
            let detected_at_micros = SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_micros() as u64)
                .unwrap_or_default();
            let signature = tx
                .signatures
                .first()
                .map(|s| s.as_ref().to_vec())
                .unwrap_or_default();
            let _ = pump_create_sender.send(PbPumpCreate {
                slot,
                mint: info.mint.to_bytes().to_vec(),
                bonding_curve: info.bonding_curve.to_bytes().to_vec(),
                associated_bonding_curve: info.associated_bonding_curve.to_bytes().to_vec(),
                creator: info.creator.to_bytes().to_vec(),
                user: info.user.to_bytes().to_vec(),
                token_program: info.token_program.to_bytes().to_vec(),
                dev_buy_lamports: info.dev_buy_lamports,
                signature,
                is_v2: info.is_v2,
                detected_at_micros,
            });
            metrics.pump_creates_emitted.fetch_add(1, Ordering::Relaxed);
        }
    }
}

/// Broadcasts the same packet to multiple recipients, parses it into a Shred if possible,
/// and stores that shred in `all_shreds`.
///
/// The batch reaches the sniper's thread before this one does anything else with it, and it
/// gets there as an `Arc` rather than a copy: cloning a `PacketBatch` is a memcpy of every
/// packet in it -- up to 80KB -- and the reconstructor cannot start until that memcpy is
/// finished. Nothing here needs to mutate the batch, so nothing here has to own one.
#[allow(clippy::too_many_arguments)]
fn recv_from_channel_and_send_multiple_dest(
    maybe_packet_batch: Result<PacketBatch, RecvError>,
    deduper: &RwLock<Deduper<2, [u8]>>,
    send_socket: &UdpSocket,
    local_dest_sockets: &[SocketAddr],
    should_reconstruct_shreds: bool,
    reconstruct_tx: &crossbeam_channel::Sender<Arc<PacketBatch>>,
    debug_trace_shred: bool,
    metrics: &ShredMetrics,
) -> Result<(), ShredstreamProxyError> {
    let packet_batch = Arc::new(maybe_packet_batch.map_err(ShredstreamProxyError::RecvError)?);
    let trace_shred_received_time = SystemTime::now();

    // first thing that happens to a batch, before metrics, dedup or forwarding
    if should_reconstruct_shreds {
        let _ = reconstruct_tx.try_send(packet_batch.clone());
    }

    metrics
        .received
        .fetch_add(packet_batch.len() as u64, Ordering::Relaxed);
    debug!(
        "Got batch of {} packets, total size in bytes: {}",
        packet_batch.len(),
        packet_batch.iter().map(|x| x.meta().size).sum::<usize>()
    );

    // Dedup by hand instead of through `dedup_packets_and_count_discards`, which needs `&mut`
    // on the batch so it can set the discard flag. Collecting the survivors is the same work
    // without the mutation, and it leaves the batch shareable.
    let mut kept: Vec<&[u8]> = Vec::with_capacity(packet_batch.len());
    let mut num_deduped = 0u64;
    // one DashMap entry per source address per batch instead of one per packet: the entry
    // API hashes the address and takes a shard lock, and a batch is almost always one source
    let mut run_addr: Option<IpAddr> = None;
    let mut run_discarded = 0u64;
    let mut run_kept = 0u64;
    {
        let deduper = deduper.read().unwrap();
        for packet in packet_batch.iter() {
            let data = packet.data(..);
            let duplicate = packet.meta().discard()
                || data.map(|d| deduper.dedup(d)).unwrap_or(true);

            let addr = packet.meta().addr;
            if run_addr != Some(addr) {
                if let Some(prev) = run_addr {
                    record_source(metrics, prev, run_discarded, run_kept);
                }
                run_addr = Some(addr);
                run_discarded = 0;
                run_kept = 0;
            }
            if duplicate {
                num_deduped += 1;
                run_discarded += 1;
            } else if let Some(data) = data {
                kept.push(data);
                run_kept += 1;
            }
        }
    }
    if let Some(prev) = run_addr {
        record_source(metrics, prev, run_discarded, run_kept);
    }
    metrics.duplicate.fetch_add(num_deduped, Ordering::Relaxed);

    // send out to RPCs
    let mut packets_with_dest: Vec<(&[u8], &SocketAddr)> = Vec::with_capacity(kept.len());
    for outgoing_socketaddr in local_dest_sockets.iter() {
        packets_with_dest.clear();
        packets_with_dest.extend(kept.iter().map(|data| (*data, outgoing_socketaddr)));

        match batch_send(send_socket, &packets_with_dest) {
            Ok(_) => {
                metrics
                    .success_forward
                    .fetch_add(packets_with_dest.len() as u64, Ordering::Relaxed);
            }
            Err(SendPktsError::IoError(err, num_failed)) => {
                metrics
                    .fail_forward
                    .fetch_add(packets_with_dest.len() as u64, Ordering::Relaxed);
                error!(
                    "Failed to send batch of size {} to {outgoing_socketaddr:?}. \
                     {num_failed} packets failed. Error: {err}",
                    packets_with_dest.len()
                );
            }
        }
    }

    // Count TraceShred shreds
    if debug_trace_shred {
        packet_batch
            .iter()
            .filter_map(|p| TraceShred::decode(p.data(..)?).ok())
            .filter(|t| t.created_at.is_some())
            .for_each(|trace_shred| {
                let elapsed = trace_shred_received_time
                    .duration_since(SystemTime::try_from(trace_shred.created_at.unwrap()).unwrap())
                    .unwrap_or_default();

                datapoint_info!(
                    "shredstream_proxy-trace_shred_latency",
                    "trace_region" => trace_shred.region,
                    ("trace_seq_num", trace_shred.seq_num as i64, i64),
                    ("elapsed_micros", elapsed.as_micros(), i64),
                );
            });
    }

    Ok(())
}

/// Folds one source address's counts into the receive-stats map.
#[inline]
fn record_source(metrics: &ShredMetrics, addr: IpAddr, discarded: u64, kept: u64) {
    if discarded == 0 && kept == 0 {
        return;
    }
    metrics
        .packets_received
        .entry(addr)
        .and_modify(|(d, nd)| {
            *d += discarded;
            *nd += kept;
        })
        .or_insert((discarded, kept));
}

/// Starts a thread that updates our destinations used by the forwarder threads
pub fn start_destination_refresh_thread(
    endpoint_discovery_url: String,
    discovered_endpoints_port: u16,
    static_dest_sockets: Vec<(SocketAddr, String)>,
    unioned_dest_sockets: Arc<ArcSwap<Vec<SocketAddr>>>,
    shutdown_receiver: Receiver<()>,
    exit: Arc<AtomicBool>,
) -> JoinHandle<()> {
    Builder::new().name("ssPxyDstRefresh".to_string()).spawn(move || {
        let fetch_socket_tick = crossbeam_channel::tick(Duration::from_secs(30));
        let metrics_tick = crossbeam_channel::tick(Duration::from_secs(30));
        let mut socket_count = static_dest_sockets.len();
        while !exit.load(Ordering::Relaxed) {
            crossbeam_channel::select! {
                    recv(fetch_socket_tick) -> _ => {
                        let fetched = fetch_unioned_destinations(
                            &endpoint_discovery_url,
                            discovered_endpoints_port,
                            &static_dest_sockets,
                        );
                        let new_sockets = match fetched {
                            Ok(s) => {
                                info!("Sending shreds to {} destinations: {s:?}", s.len());
                                s
                            }
                            Err(e) => {
                                warn!("Failed to fetch from discovery service, retrying. Error: {e}");
                                datapoint_warn!("shredstream_proxy-destination_refresh_error",
                                                ("prev_unioned_dest_count", socket_count, i64),
                                                ("errors", 1, i64),
                                                ("error_str", e.to_string(), String),
                                );
                                continue;
                            }
                        };
                        socket_count = new_sockets.len();
                        unioned_dest_sockets.store(Arc::new(new_sockets));
                    }
                    recv(metrics_tick) -> _ => {
                        datapoint_info!("shredstream_proxy-destination_refresh_stats",
                                        ("destination_count", socket_count, i64),
                        );
                    }
                    recv(shutdown_receiver) -> _ => {
                        break;
                    }
                }
        }
    }).unwrap()
}

/// Returns dynamically discovered endpoints with CLI arg defined endpoints
fn fetch_unioned_destinations(
    endpoint_discovery_url: &str,
    discovered_endpoints_port: u16,
    static_dest_sockets: &[(SocketAddr, String)],
) -> Result<Vec<SocketAddr>, ShredstreamProxyError> {
    let bytes = reqwest::blocking::get(endpoint_discovery_url)?.bytes()?;

    let sockets_json = match serde_json::from_slice::<Vec<IpAddr>>(&bytes) {
        Ok(s) => s,
        Err(e) => {
            warn!(
                "Failed to parse json from: {:?}",
                std::str::from_utf8(&bytes)
            );
            return Err(ShredstreamProxyError::from(e));
        }
    };

    // resolve again since ip address could change
    let static_dest_sockets = static_dest_sockets
        .iter()
        .filter_map(|(_socketaddr, hostname_port)| {
            Some(resolve_hostname_port(hostname_port).ok()?.0)
        })
        .collect::<Vec<_>>();

    let unioned_dest_sockets = sockets_json
        .into_iter()
        .map(|ip| SocketAddr::new(ip, discovered_endpoints_port))
        .chain(static_dest_sockets)
        .unique()
        .collect::<Vec<SocketAddr>>();
    Ok(unioned_dest_sockets)
}

/// Reset dedup + send metrics to influx
pub fn start_forwarder_accessory_thread(
    deduper: Arc<RwLock<Deduper<2, [u8]>>>,
    metrics: Arc<ShredMetrics>,
    metrics_update_interval_ms: u64,
    shutdown_receiver: Receiver<()>,
    exit: Arc<AtomicBool>,
) -> JoinHandle<()> {
    Builder::new()
        .name("ssPxyAccessory".to_string())
        .spawn(move || {
            let metrics_tick =
                crossbeam_channel::tick(Duration::from_millis(metrics_update_interval_ms));
            let deduper_reset_tick = crossbeam_channel::tick(Duration::from_secs(2));
            let mut rng = rand::thread_rng();
            while !exit.load(Ordering::Relaxed) {
                crossbeam_channel::select! {
                    // reset deduper to avoid false positives
                    recv(deduper_reset_tick) -> _ => {
                        deduper
                            .write()
                            .unwrap()
                            .maybe_reset(&mut rng, DEDUPER_FALSE_POSITIVE_RATE, DEDUPER_RESET_CYCLE);
                    }

                    // send metrics to influx
                    recv(metrics_tick) -> _ => {
                        metrics.report();
                        metrics.reset();
                    }

                    // handle SIGINT shutdown
                    recv(shutdown_receiver) -> _ => {
                        break;
                    }
                }
            }
        })
        .unwrap()
}

pub struct ShredMetrics {
    // receive stats
    /// Total number of shreds received. Includes duplicates when receiving shreds from multiple regions
    pub received: AtomicU64,
    /// Total number of shreds successfully forwarded, accounting for all destinations
    pub success_forward: AtomicU64,
    /// Total number of shreds failed to forward, accounting for all destinations
    pub fail_forward: AtomicU64,
    /// Number of duplicate shreds received
    pub duplicate: AtomicU64,
    /// (discarded, not discarded, from other shredstream instances)
    pub packets_received: DashMap<IpAddr, (u64, u64)>,

    // service metrics
    pub enabled_grpc_service: bool,
    /// Number of data shreds recovered using coding shreds
    pub recovered_count: AtomicU64,
    /// Number of Solana entries decoded from shreds
    pub entry_count: AtomicU64,
    /// Number of transactions decoded from shreds
    pub txn_count: AtomicU64,
    /// Number of times we couldn't find the previous DATA_COMPLETE_SHRED flag
    pub unknown_start_position_count: AtomicU64,
    /// Number of FEC recovery errors
    pub fec_recovery_error_count: AtomicU64,
    /// Number of bincode Entry deserialization errors
    pub bincode_deserialize_error_count: AtomicU64,
    /// Number of times we couldn't find the previous DATA_COMPLETE_SHRED flag but tried to deshred+deserialize, and failed
    pub unknown_start_position_error_count: AtomicU64,
    /// Number of pump.fun `create` / `create_v2` instructions decoded from shreds
    pub pump_creates_seen: AtomicU64,
    /// Number of pump.fun creates published on the gRPC feed
    pub pump_creates_emitted: AtomicU64,
    /// Number of launches the sniper actually fired on
    pub snipes_fired: AtomicU64,
    /// Segments whose bincode deserialize was skipped because no create discriminator was present
    pub deserialize_skipped_count: AtomicU64,
    /// Creates found in a partial segment, before the segment was complete
    pub early_creates_count: AtomicU64,

    // cumulative metrics (persist after reset)
    pub agg_received_cumulative: AtomicU64,
    pub agg_success_forward_cumulative: AtomicU64,
    pub agg_fail_forward_cumulative: AtomicU64,
    pub duplicate_cumulative: AtomicU64,
}

impl Default for ShredMetrics {
    fn default() -> Self {
        Self::new(false)
    }
}

impl ShredMetrics {
    pub fn new(enabled_grpc_service: bool) -> Self {
        Self {
            enabled_grpc_service,
            received: Default::default(),
            success_forward: Default::default(),
            fail_forward: Default::default(),
            duplicate: Default::default(),
            packets_received: DashMap::with_capacity(10),
            recovered_count: Default::default(),
            entry_count: Default::default(),
            txn_count: Default::default(),
            unknown_start_position_count: Default::default(),
            fec_recovery_error_count: Default::default(),
            bincode_deserialize_error_count: Default::default(),
            pump_creates_seen: Default::default(),
            pump_creates_emitted: Default::default(),
            snipes_fired: Default::default(),
            deserialize_skipped_count: Default::default(),
            early_creates_count: Default::default(),
            unknown_start_position_error_count: Default::default(),
            agg_received_cumulative: Default::default(),
            agg_success_forward_cumulative: Default::default(),
            agg_fail_forward_cumulative: Default::default(),
            duplicate_cumulative: Default::default(),
        }
    }

    pub fn report(&self) {
        datapoint_info!(
            "shredstream_proxy-connection_metrics",
            ("received", self.received.load(Ordering::Relaxed), i64),
            (
                "success_forward",
                self.success_forward.load(Ordering::Relaxed),
                i64
            ),
            (
                "fail_forward",
                self.fail_forward.load(Ordering::Relaxed),
                i64
            ),
            ("duplicate", self.duplicate.load(Ordering::Relaxed), i64),
        );

        if self.enabled_grpc_service {
            datapoint_info!(
                "shredstream_proxy-service_metrics",
                (
                    "recovered_count",
                    self.recovered_count.swap(0, Ordering::Relaxed),
                    i64
                ),
                (
                    "entry_count",
                    self.entry_count.swap(0, Ordering::Relaxed),
                    i64
                ),
                ("txn_count", self.txn_count.swap(0, Ordering::Relaxed), i64),
                (
                    "unknown_start_position_count",
                    self.unknown_start_position_count.swap(0, Ordering::Relaxed),
                    i64
                ),
                (
                    "fec_recovery_error_count",
                    self.fec_recovery_error_count.swap(0, Ordering::Relaxed),
                    i64
                ),
                (
                    "bincode_deserialize_error_count",
                    self.bincode_deserialize_error_count
                        .swap(0, Ordering::Relaxed),
                    i64
                ),
                (
                    "pump_creates_seen",
                    self.pump_creates_seen.swap(0, Ordering::Relaxed),
                    i64
                ),
                (
                    "pump_creates_emitted",
                    self.pump_creates_emitted.swap(0, Ordering::Relaxed),
                    i64
                ),
                (
                    "snipes_fired",
                    self.snipes_fired.swap(0, Ordering::Relaxed),
                    i64
                ),
                (
                    "deserialize_skipped_count",
                    self.deserialize_skipped_count.swap(0, Ordering::Relaxed),
                    i64
                ),
                // stays at zero unless pump.fun moves a create account behind a lookup
                // table, at which point launches would go silently undetected
                (
                    "early_creates_count",
                    self.early_creates_count.swap(0, Ordering::Relaxed),
                    i64
                ),
                (
                    "unresolved_creates",
                    pumpfun::UNRESOLVED_CREATES.swap(0, Ordering::Relaxed),
                    i64
                ),
                (
                    "unknown_start_position_error_count",
                    self.unknown_start_position_error_count
                        .swap(0, Ordering::Relaxed),
                    i64
                ),
            );
        }

        self.packets_received
            .retain(|addr, (discarded_packets, not_discarded_packets)| {
                datapoint_info!("shredstream_proxy-receiver_stats",
                    "addr" => addr.to_string(),
                    ("discarded_packets", *discarded_packets, i64),
                    ("not_discarded_packets", *not_discarded_packets, i64),
                );
                false
            });
    }

    /// resets current values, increments cumulative values
    pub fn reset(&self) {
        self.agg_received_cumulative
            .fetch_add(self.received.swap(0, Ordering::Relaxed), Ordering::Relaxed);
        self.agg_success_forward_cumulative.fetch_add(
            self.success_forward.swap(0, Ordering::Relaxed),
            Ordering::Relaxed,
        );
        self.agg_fail_forward_cumulative.fetch_add(
            self.fail_forward.swap(0, Ordering::Relaxed),
            Ordering::Relaxed,
        );
        self.duplicate_cumulative
            .fetch_add(self.duplicate.swap(0, Ordering::Relaxed), Ordering::Relaxed);
    }
}

#[cfg(test)]
mod tests {
    use std::{
        net::{IpAddr, Ipv4Addr, SocketAddr, UdpSocket},
        str::FromStr,
        sync::{Arc, Mutex, RwLock},
        thread,
        thread::sleep,
        time::Duration,
    };

    use solana_perf::{
        deduper::Deduper,
        packet::{Meta, Packet, PacketBatch},
    };
    use solana_sdk::packet::{PacketFlags, PACKET_DATA_SIZE};

    use crate::forwarder::{fill_from_fired, recv_from_channel_and_send_multiple_dest, ShredMetrics};

    /// Pins the seller's wire contract: the Fill's `max_sol_cost` field is the
    /// instruction-independent cost basis in lamports and `amount` the expected token
    /// fill — never the raw buy-instruction args. Under `buy_exact_sol_in` those args are
    /// (sol_in, min_tokens_out); forwarding them verbatim gave the Node seller a token
    /// count as its cost basis, its value multiple read ~0, and the 0.8x stop dumped
    /// every position at slot +1.
    #[test]
    fn fill_carries_cost_basis_and_expected_tokens_not_the_wire_args() {
        use solana_sdk::pubkey::Pubkey;
        let fired = sniper::FiredLaunch {
            mint: Pubkey::new_from_array([1; 32]),
            token_account: Pubkey::new_from_array([2; 32]),
            seed: *b"001234",
            // exact_sol_in-shaped wire args: arg0 = sol_in, arg1 = token floor
            amount: 2_000_000_000,
            max_sol_cost: 60_000_000_000_000,
            cost_lamports: 2_000_000_000,
            expected_tokens: 64_000_000_000_000,
            bonding_curve: Pubkey::new_from_array([3; 32]),
            associated_bonding_curve: Pubkey::new_from_array([4; 32]),
            creator: Pubkey::new_from_array([5; 32]),
            token_program: Pubkey::new_from_array([6; 32]),
        };
        let fill = fill_from_fired(7, &fired);
        assert_eq!(fill.slot, 7);
        assert_eq!(fill.max_sol_cost, fired.cost_lamports, "lamports, not the token floor");
        assert_eq!(fill.amount, fired.expected_tokens, "tokens, not sol_in");
        assert_eq!(fill.mint, fired.mint.to_bytes().to_vec());
        assert_eq!(fill.seed, "001234");
    }

    fn listen_and_collect(listen_socket: UdpSocket, received_packets: Arc<Mutex<Vec<Vec<u8>>>>) {
        let mut buf = [0u8; PACKET_DATA_SIZE];
        loop {
            listen_socket.recv(&mut buf).unwrap();
            received_packets.lock().unwrap().push(Vec::from(buf));
        }
    }

    #[test]
    fn test_2shreds_3destinations() {
        let packet_batch = PacketBatch::new(vec![
            Packet::new(
                [1; PACKET_DATA_SIZE],
                Meta {
                    size: PACKET_DATA_SIZE,
                    addr: IpAddr::V4(Ipv4Addr::UNSPECIFIED),
                    port: 48289, // received on random port
                    flags: PacketFlags::empty(),
                },
            ),
            Packet::new(
                [2; PACKET_DATA_SIZE],
                Meta {
                    size: PACKET_DATA_SIZE,
                    addr: IpAddr::V4(Ipv4Addr::UNSPECIFIED),
                    port: 9999,
                    flags: PacketFlags::empty(),
                },
            ),
        ]);
        let (packet_sender, packet_receiver) = crossbeam_channel::unbounded::<PacketBatch>();
        packet_sender.send(packet_batch).unwrap();

        let dest_socketaddrs = vec![
            SocketAddr::from_str("0.0.0.0:32881").unwrap(),
            SocketAddr::from_str("0.0.0.0:33881").unwrap(),
            SocketAddr::from_str("0.0.0.0:34881").unwrap(),
        ];

        let test_listeners = dest_socketaddrs
            .iter()
            .map(|socketaddr| {
                (
                    UdpSocket::bind(socketaddr).unwrap(),
                    *socketaddr,
                    // store results in vec of packet, where packet is Vec<u8>
                    Arc::new(Mutex::new(vec![])),
                )
            })
            .collect::<Vec<_>>();

        let udp_sender = UdpSocket::bind("0.0.0.0:10000").unwrap();

        // spawn listeners
        test_listeners
            .iter()
            .for_each(|(listen_socket, _socketaddr, to_receive)| {
                let socket = listen_socket.try_clone().unwrap();
                let to_receive = to_receive.to_owned();
                thread::spawn(move || listen_and_collect(socket, to_receive));
            });

        let (reconstruct_tx, _reconstruct_rx) = crossbeam_channel::bounded(10_240);
        // send packets
        recv_from_channel_and_send_multiple_dest(
            packet_receiver.recv(),
            &Arc::new(RwLock::new(Deduper::<2, [u8]>::new(
                &mut rand::thread_rng(),
                crate::forwarder::DEDUPER_NUM_BITS,
            ))),
            &udp_sender,
            &Arc::new(dest_socketaddrs),
            true,
            &reconstruct_tx,
            false,
            &Arc::new(ShredMetrics::default()),
        )
        .unwrap();

        // allow packets to be received
        sleep(Duration::from_millis(500));

        let received = test_listeners
            .iter()
            .map(|(_, _, results)| results.clone())
            .collect::<Vec<_>>();

        // check results
        for received in received.iter() {
            let received = received.lock().unwrap();
            assert_eq!(received.len(), 2);
            assert!(received
                .iter()
                .all(|packet| packet.len() == PACKET_DATA_SIZE));
            assert_eq!(received[0], [1; PACKET_DATA_SIZE]);
            assert_eq!(received[1], [2; PACKET_DATA_SIZE]);
        }

        assert_eq!(
            received
                .iter()
                .fold(0, |acc, elem| acc + elem.lock().unwrap().len()),
            6
        );
    }
}
