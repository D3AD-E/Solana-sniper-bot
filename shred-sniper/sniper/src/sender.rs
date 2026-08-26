//! Provider senders.
//!
//! One thread per provider, each owning a warm, persistent connection to *every* regional
//! endpoint that provider publishes. The detect thread only memcpys a prepatched
//! transaction into a bounded channel; signing, base64 and the socket writes all happen
//! here, so a slow or dead provider can never stall deshredding.
//!
//! Fanning out to every region costs nothing extra: all variants of a launch carry the same
//! durable nonce, so whichever endpoint lands first advances it and the rest become
//! invalid — one tip is paid no matter how many endpoints were tried.
//!
//! The transaction is signed **once** per provider and the same bytes go to all of its
//! endpoints. Signing per endpoint would multiply a ~20µs ed25519 operation by the number
//! of regions for no benefit.

use std::{
    io::{Read, Write},
    net::{TcpStream, ToSocketAddrs},
    sync::{
        atomic::{AtomicU64, Ordering},
        Arc,
    },
    thread::{Builder, JoinHandle},
    time::{Duration, Instant},
};

use base64::{engine::general_purpose::STANDARD, Engine};
use crossbeam_channel::{Receiver, RecvTimeoutError, Sender, TryRecvError};
use ed25519_dalek::{Signer, SigningKey};
use log::{debug, info, warn};

use crate::{
    batch_write::BatchWriter,
    config::{BodyFormat, ProviderConfig},
    template::MSG_OFFSET,
};

/// Max serialized transaction size on Solana.
pub const MAX_TX: usize = 1232;

/// The body of one launch's buy: everything patched except the three fields that differ
/// per provider. Shared by every provider variant of that launch, so the detect thread
/// copies the transaction **once** no matter how many providers are configured.
pub struct TxBuf {
    pub len: u16,
    pub tx: [u8; MAX_TX],
}

impl Default for TxBuf {
    fn default() -> Self {
        Self {
            len: 0,
            tx: [0u8; MAX_TX],
        }
    }
}

/// Byte offsets of the three fields a provider owns. Constant for the process; handed to
/// each sender at spawn so the patching can happen on the sender's own thread.
#[derive(Debug, Clone, Copy)]
pub struct TipOffsets {
    pub tip_account: usize,
    pub tip_lamports: usize,
    pub cu_price: usize,
}

/// One provider's variant of a launch.
///
/// Carrying the body behind an `Arc` instead of inline turns the fan-out from `N` copies of
/// 1232 bytes into one copy plus `N` refcount bumps. That matters twice: it takes ~1.2µs per
/// provider off the detect thread, and it stops the last provider from being queued ~10µs
/// after the first — and which provider wins the race is not something we get to know.
#[derive(Clone)]
pub struct Job {
    pub base: Arc<TxBuf>,
    pub tip_account: [u8; 32],
    pub tip_lamports: u64,
    pub cu_price: u64,
}

#[derive(Default, Debug)]
pub struct SenderMetrics {
    /// bytes handed to the socket. This is NOT "the provider took it" — see `rejected`.
    pub sent: AtomicU64,
    pub send_errors: AtomicU64,
    pub reconnects: AtomicU64,
    pub dropped_full: AtomicU64,
    /// submits the provider answered with a non-2xx status: a rotated or expired key, a rate
    /// limit, a tip under their floor, a body format they stopped accepting. Every one of
    /// these used to be discarded unread, so a provider could reject the entire fan-out while
    /// the sniper reported a clean run.
    pub rejected: AtomicU64,
    /// launches where an endpoint had no warm connection and was skipped rather than
    /// reconnected inline. A reconnect costs a TCP handshake the launch cannot wait for; a
    /// steady rate here means connections are dying between launches.
    pub skipped_cold: AtomicU64,
}

pub struct ProviderHandle {
    pub name: String,
    pub tx: Sender<Job>,
    pub metrics: Arc<SenderMetrics>,
    /// provider tip accounts, raw bytes so the hot path patches without conversion.
    /// Rotated per launch the way live senders spread tips across accounts.
    pub tip_accounts: Vec<[u8; 32]>,
    pub tip_lamports: u64,
    pub cu_price: u64,
    pub endpoint_count: usize,
}

impl ProviderHandle {
    /// Hot path: never blocks. A full queue means the provider is wedged; drop and count.
    #[inline(always)]
    pub fn try_send(&self, job: Job) {
        if self.tx.try_send(job).is_err() {
            self.metrics.dropped_full.fetch_add(1, Ordering::Relaxed);
        }
    }
}

enum Conn {
    Plain(TcpStream),
    Tls(Box<rustls::StreamOwned<rustls::ClientConnection, TcpStream>>),
    /// A `connect`ed datagram socket (falcon `:9000`). `connect` here only fixes the peer so
    /// that an ordinary `write` becomes a `send`; nothing is negotiated and there is no
    /// stream. It is never read from — the provider does not answer.
    Udp(std::net::UdpSocket),
}

impl Conn {
    fn write_all(&mut self, buf: &[u8]) -> std::io::Result<()> {
        match self {
            Conn::Plain(s) => s.write_all(buf).and_then(|_| s.flush()),
            Conn::Tls(s) => s.write_all(buf).and_then(|_| s.flush()),
            // one datagram, all or nothing: a partial send is not a thing here, and a short
            // count would mean a truncated transaction rather than something to finish later
            Conn::Udp(s) => s.send(buf).and_then(|n| {
                if n == buf.len() {
                    Ok(())
                } else {
                    Err(std::io::Error::new(
                        std::io::ErrorKind::WriteZero,
                        "short datagram",
                    ))
                }
            }),
        }
    }
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        match self {
            Conn::Plain(s) => s.read(buf),
            Conn::Tls(s) => s.read(buf),
            // nothing ever arrives; claiming "no data ready" keeps the drain loop honest
            // without ever marking the socket dead
            Conn::Udp(_) => Err(std::io::Error::from(std::io::ErrorKind::WouldBlock)),
        }
    }
    fn set_nonblocking(&self, on: bool) -> std::io::Result<()> {
        match self {
            Conn::Plain(s) => s.set_nonblocking(on),
            Conn::Tls(s) => s.sock.set_nonblocking(on),
            Conn::Udp(s) => s.set_nonblocking(on),
        }
    }
}

/// True when a read on a non-blocking socket simply had nothing ready.
#[inline]
fn nothing_ready(e: &std::io::Error) -> bool {
    matches!(
        e.kind(),
        std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut
    )
}

/// Reads whatever the provider has already sent, without ever waiting for it.
///
/// The response to a submit arrives a whole network RTT after the write. Reading it with the
/// ordinary blocking socket parks the sender thread for that RTT on *every endpoint in turn*
/// — eight regions at 20ms is 160ms during which a launch sitting in the queue does not get
/// sent at all. So the drain never blocks: it takes what the kernel already has and leaves
/// the rest for the next launch's drain or the keep-alive tick.
fn drain_ready(ep: &mut Endpoint, buf: &mut [u8], metrics: &SenderMetrics) {
    let Some(conn) = ep.conn.as_mut() else { return };
    // a datagram provider never answers; toggling the socket to look for a reply that cannot
    // arrive is two syscalls per launch for nothing
    if matches!(conn, Conn::Udp(_)) {
        return;
    }
    if conn.set_nonblocking(true).is_err() {
        ep.conn = None;
        return;
    }
    // status codes are collected while the socket is borrowed and applied afterwards, since
    // attributing them needs `ep` mutably too
    let mut codes = StatusCodes::default();
    let mut dead = false;
    loop {
        match conn.read(buf) {
            // a keep-alive connection only reports 0 bytes when the peer closed it
            Ok(0) => {
                dead = true;
                break;
            }
            // a full buffer means there may be more behind it
            Ok(n) if n == buf.len() => {
                codes.scan(&buf[..n]);
                continue;
            }
            Ok(n) => {
                codes.scan(&buf[..n]);
                break;
            }
            Err(e) if nothing_ready(&e) => break,
            Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
            Err(_) => {
                dead = true;
                break;
            }
        }
    }
    if dead || conn.set_nonblocking(false).is_err() {
        ep.conn = None;
    }
    note_status(&codes, ep, metrics);
}

/// The status codes found in one drain. Fixed capacity and no allocation: a drain normally
/// carries nought or one reply, and overflowing simply means the extras go uncounted, which is
/// the right failure for a health signal.
#[derive(Default)]
struct StatusCodes {
    codes: [[u8; 3]; 8],
    n: usize,
}

impl StatusCodes {
    fn scan(&mut self, buf: &[u8]) {
        let mut rest = buf;
        // a pipelined drain can hand back several responses at once
        while let Some(pos) = find_status_line(rest) {
            if self.n < self.codes.len() {
                self.codes[self.n].copy_from_slice(&rest[pos + 9..pos + 12]);
                self.n += 1;
            }
            rest = &rest[pos + 12..];
        }
    }

    fn iter(&self) -> impl Iterator<Item = &[u8; 3]> {
        self.codes[..self.n].iter()
    }
}

/// Counts a provider's rejections, which are otherwise completely invisible.
///
/// `metrics.sent` only ever meant "the bytes left our socket". Everything that happens after
/// that — an expired or rotated key (401/403), a rate limit (429, or 419 at 0slot), a tip
/// below the provider's floor, a malformed body after a wire-format change — comes back as an
/// ordinary HTTP response on a healthy connection, and the drain used to discard it. The
/// result was a sniper that could report a full fan-out while landing nothing at all, with no
/// signal anywhere. Nozomi are explicit about the worst version of this: transactions tipping
/// under the minimum "are silently dropped".
///
/// This is not a parser. It reads status lines, which is enough to tell "accepted" from
/// "rejected" and to name the class in a log line.
///
/// Two known blind spots, both verified against the live providers:
///   * **flashblock answers HTTP 200 for its own errors** and puts the failure in the JSON
///     body (`{"code":403,"message":"PermissionDenied","success":false}` for a bad key,
///     `1015 Transaction has bad format` for a bad transaction). Its rejections are therefore
///     invisible here. Reading them needs a body parse, which is more than a health signal
///     on the response path is worth.
///   * **node1 answers 605**, not a 4xx, when the key is good and the transaction is bad.
///     That counts as a rejection, which is correct — but it means node1's counter moves for
///     transaction faults as well as auth faults.
///
/// **Keep-alive replies must not be counted.** jito publishes no health endpoint and 404s
/// every path; nextblock's `/health` 401s for our key. Both are deliberate and documented
/// here, and both would otherwise report a rejection every `KEEPALIVE_SECS` forever. The
/// probe reply also does not arrive before the drain that immediately follows the probe
/// write — it is a network round trip away — so it is normally picked up by the *next*
/// drain, which is usually a launch drain. A plain counter would therefore mis-attribute it
/// to a submit. HTTP/1.1 responses on a kept-alive connection come back strictly in request
/// order, so `ep.awaiting` records what each pending reply belongs to and this pops them in
/// the same order.
fn note_status(codes: &StatusCodes, ep: &mut Endpoint, metrics: &SenderMetrics) {
    for code in codes.iter() {
        // oldest outstanding request wins; an empty queue means we lost sync (a truncated
        // read split a status line) and the safe reading is "not a submit"
        let was_probe = ep.awaiting.pop_front().unwrap_or(true);
        if was_probe {
            continue;
        }
        if code.starts_with(b"2") {
            ep.consecutive_rejects = 0;
            continue;
        }
        metrics.rejected.fetch_add(1, Ordering::Relaxed);
        ep.consecutive_rejects = ep.consecutive_rejects.saturating_add(1);
        // one rejection is noise (a single 429 in a burst); a run of them on one endpoint is
        // a dead key, a tip under the floor, or a body format the provider stopped taking
        if ep.consecutive_rejects == 1 || ep.consecutive_rejects % 32 == 0 {
            let code = std::str::from_utf8(&code[..]).unwrap_or("???");
            warn!(
                "{}: HTTP {code} from {} ({} in a row) - check key, tip floor and body format",
                ep.provider, ep.host, ep.consecutive_rejects
            );
        }
    }
}

/// Offset of the next `HTTP/1.x NNN` status line, if the buffer holds a complete one.
fn find_status_line(buf: &[u8]) -> Option<usize> {
    buf.windows(12)
        .position(|w| w.starts_with(b"HTTP/1.") && w[9..12].iter().all(|c| c.is_ascii_digit()))
}

fn tls_config() -> Arc<rustls::ClientConfig> {
    let mut roots = rustls::RootCertStore::empty();
    roots.extend(webpki_roots::TLS_SERVER_ROOTS.iter().cloned());
    Arc::new(
        rustls::ClientConfig::builder()
            .with_root_certificates(roots)
            .with_no_client_auth(),
    )
}

/// One regional endpoint of a provider: its own socket, its own request head.
struct Endpoint {
    /// provider name, carried for log lines
    provider: String,
    host: String,
    port: u16,
    tls: bool,
    udp: bool,
    /// Resolved once at startup and reused. Name resolution used to happen inside `connect`,
    /// which is called from the launch path — see `resolve`.
    addr: Option<std::net::SocketAddr>,
    /// everything up to Content-Length, precomputed
    head: Vec<u8>,
    health: Vec<u8>,
    conn: Option<Conn>,
    /// this endpoint's fully built request, kept alive across a batched submit
    request: Vec<u8>,
    /// what each not-yet-read reply belongs to, oldest first: `true` = keep-alive probe,
    /// `false` = a submitted transaction. See `note_status`.
    awaiting: std::collections::VecDeque<bool>,
    consecutive_rejects: u32,
}

/// Bound on `awaiting` so a peer that stops replying cannot grow it without limit. Well above
/// anything real: replies are drained after every launch and every probe.
const MAX_AWAITING: usize = 64;

impl Endpoint {
    /// Plain TCP endpoints can go through the batch writer; TLS cannot, because rustls owns
    /// the record framing.
    #[cfg(target_os = "linux")]
    fn raw_fd(&self) -> Option<std::os::fd::RawFd> {
        use std::os::fd::AsRawFd;
        match self.conn.as_ref()? {
            Conn::Plain(s) => Some(s.as_raw_fd()),
            // a connected datagram socket takes an ordinary write, so falcon's regions can
            // leave in one io_uring_enter exactly like a plain TCP provider's
            Conn::Udp(s) => Some(s.as_raw_fd()),
            Conn::Tls(_) => None,
        }
    }

    fn build_request(&mut self, length_digits: &[u8], body: &[u8]) {
        self.request.clear();
        // a datagram carries the payload and nothing else — no head, no framing, no length
        if self.udp {
            self.request.extend_from_slice(body);
            return;
        }
        self.request.extend_from_slice(&self.head);
        self.request.extend_from_slice(b"Content-Length: ");
        self.request.extend_from_slice(length_digits);
        self.request.extend_from_slice(b"\r\n\r\n");
        self.request.extend_from_slice(body);
    }
}

/// How long a connect attempt may take when it is made from the launch path.
///
/// This used to be 5 seconds, and name resolution sat in front of it. Both ran inline in the
/// launch loop, for every endpoint in turn, before a single byte of the transaction was
/// written — so one blackholed endpoint stalled that provider's entire fan-out, and the
/// serial retry after the batch could pay it twice. blockrazor has eleven endpoints on one
/// thread. A create-block snipe is dead long before any of that returns.
///
/// A launch now never waits for a reconnect at all (see `run`), so this bound only applies to
/// the startup warm-up and the keep-alive tick, where blocking is harmless. It stays short
/// anyway: an endpoint that cannot complete a handshake in 2s is not going to win a race.
const CONNECT_TIMEOUT: Duration = Duration::from_secs(2);

impl Endpoint {
    /// Resolves the hostname and caches the result. Called at startup and, off the hot path,
    /// whenever a reconnect finds no cached address.
    ///
    /// `to_socket_addrs` is a blocking getaddrinfo: a cold cache or a slow resolver makes it
    /// tens of milliseconds, and on a failure it can be far worse. Doing it once here is the
    /// whole point — the launch path must never call it.
    ///
    /// Note for whoever deploys this: bloXroute picks a datacenter from the EDNS client
    /// subnet, so *which* resolver the box uses changes which POP these addresses point at.
    /// Resolving once at startup makes that choice sticky for the process lifetime, which is
    /// what we want, but it also means a box pointed at Cloudflare 1.1.1.1 pins the wrong POP
    /// until restart. See INFRA.md.
    fn resolve(&mut self) -> std::io::Result<std::net::SocketAddr> {
        if let Some(a) = self.addr {
            return Ok(a);
        }
        let addr = (self.host.as_str(), self.port)
            .to_socket_addrs()?
            .next()
            .ok_or_else(|| std::io::Error::new(std::io::ErrorKind::NotFound, "no address"))?;
        self.addr = Some(addr);
        Ok(addr)
    }

    fn connect(&mut self) -> std::io::Result<()> {
        let addr = self.resolve()?;

        if self.udp {
            // an unbound ephemeral port on the matching family; `connect` fixes the peer so
            // `send` needs no address and the kernel can report ICMP errors back to us
            let bind: std::net::SocketAddr = if addr.is_ipv4() {
                "0.0.0.0:0".parse().unwrap()
            } else {
                "[::]:0".parse().unwrap()
            };
            let sock = std::net::UdpSocket::bind(bind)?;
            sock.connect(addr)?;
            self.conn = Some(Conn::Udp(sock));
            return Ok(());
        }

        let tcp = TcpStream::connect_timeout(&addr, CONNECT_TIMEOUT)?;
        tcp.set_nodelay(true)?;
        tcp.set_read_timeout(Some(Duration::from_millis(1500)))?;
        tcp.set_write_timeout(Some(Duration::from_millis(1500)))?;

        self.conn = Some(if self.tls {
            let name = rustls_pki_types::ServerName::try_from(self.host.clone())
                .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidInput, e))?;
            let client =
                rustls::ClientConnection::new(tls_config(), name).map_err(std::io::Error::other)?;
            Conn::Tls(Box::new(rustls::StreamOwned::new(client, tcp)))
        } else {
            Conn::Plain(tcp)
        });
        // a fresh connection has no history; anything still queued belongs to a socket that
        // no longer exists
        self.awaiting.clear();
        Ok(())
    }

    /// One blocking request/response round trip, for the boot-time latency gate only.
    ///
    /// Deliberately blocking and deliberately not on any hot path: this runs once, before the
    /// first launch can arrive, and it is the only place in this module that waits for a
    /// reply. The status does not matter — several providers answer the probe 404 or 405 by
    /// design — only the time and the fact that something came back.
    fn probe_rtt(&mut self, buf: &mut [u8]) -> Option<Duration> {
        if self.health.is_empty() || self.conn.is_none() {
            return None;
        }
        let health = std::mem::take(&mut self.health);
        let started = Instant::now();
        let conn = self.conn.as_mut()?;
        let out = conn.write_all(&health).and_then(|()| conn.read(buf));
        self.health = health;
        match out {
            // a 0-byte read is a closed peer, not a fast one
            Ok(0) => {
                self.conn = None;
                None
            }
            Ok(_) => Some(started.elapsed()),
            Err(_) => {
                self.conn = None;
                None
            }
        }
    }

    /// Records that one more reply is outstanding, so `note_status` can tell a probe reply
    /// from a submit reply.
    fn expect_reply(&mut self, probe: bool) {
        if self.awaiting.len() >= MAX_AWAITING {
            // the peer has stopped answering; drop the history rather than grow forever
            self.awaiting.clear();
        }
        self.awaiting.push_back(probe);
    }
}

/// How long the sender sits idle before probing every endpoint to keep its connection warm.
///
/// Set by the **tightest** provider window, not by a round number. Measured, not assumed
/// (`scripts/ping_providers.py --reuse-after N` holds a real connection open and probes again):
///
/// | provider        | idle window |
/// |-----------------|-------------|
/// | helius-sender   | **10 s**    |
/// | flashblock      | 30 s        |
/// | nozomi          | 65 s        |
/// | the rest        | longer      |
///
/// helius-sender is the binding constraint by a wide margin, and it is easy to miss because a
/// dead warm connection is not an error — the sender just reconnects, on the hot path, which
/// is exactly the handshake the warm connection exists to avoid.
///
/// The probe deadline is absolute (`next_probe` in `run`), so this is the real worst-case gap
/// between two probes on an idle endpoint. It did not used to be: the timeout was armed only
/// after the spin window expired, which silently added `sender_spin_micros` — 2 s in `.env` —
/// to every gap and pushed the real interval to ~8 s. That cleared helius-sender's measured
/// 10 s drop by very little and blew straight through the 5 s they actually document
/// ("use connection warming when your application has gaps longer than 5 seconds"). 4 s is
/// inside the documented figure with the margin the measurement suggests, and it no longer
/// moves when someone retunes the spin window.
///
/// The probe itself is cheap: one small GET per endpoint, written without waiting for the
/// replies.
const KEEPALIVE_SECS: u64 = 4;

const JSON_RPC_PREFIX: &[u8] =
    br#"{"jsonrpc":"2.0","id":1,"method":"sendTransaction","params":[""#;
const JSON_RPC_SUFFIX: &[u8] = br#"",{"encoding":"base64","skipPreflight":true,"maxRetries":0}]}"#;

// nextblock / bloxroute
const WRAPPED_PREFIX: &[u8] = br#"{"transaction":{"content":""#;
const WRAPPED_SUFFIX: &[u8] = br#""},"skipPreFlight":true,"frontRunningProtection":false}"#;

// bloxroute, with their leader-risk hold turned off and staked submission turned on.
//
// `submitProtection`: SP_MEDIUM (the default when the field is absent) waits for four
// consecutive safe slots when it scores the current or next-3 leader as high-risk; we cannot
// afford that. `"low"` is rejected as unparseable — the enum spelling is the one that works.
//
// `useStakedRPCs`: this is the field that earns bloxroute its race slot. Their docs describe
// SP_LOW as "no MEV protection; direct submission to Jito" — so with SP_LOW alone, bloxroute
// is a slower path to a block engine we already hit directly, plus a hop. `useStakedRPCs`
// switches it to weighted-stake QoS submission straight to the leader, which is a route
// nothing else in the table gives us. It has exactly two documented preconditions, both of
// which this body already met before the field was added: a tip of at least 0.001 SOL (the
// catalogue's `min_tip` for bloxroute, floored again per-provider in lib.rs) and
// `frontRunningProtection: false`.
const WRAPPED_BLOX_SUFFIX: &[u8] = br#""},"skipPreFlight":true,"frontRunningProtection":false,"useStakedRPCs":true,"submitProtection":"SP_LOW"}"#;

// lucum / blockrazor
const PLAIN_PREFIX: &[u8] = br#"{"transaction":""#;
const PLAIN_SUFFIX: &[u8] = br#""}"#;

// flashblock
const BATCH_PREFIX: &[u8] = br#"{"transactions":[""#;
const BATCH_SUFFIX: &[u8] = br#""]}"#;

fn body_wrappers(format: BodyFormat) -> (&'static [u8], &'static [u8]) {
    match format {
        BodyFormat::JsonRpc => (JSON_RPC_PREFIX, JSON_RPC_SUFFIX),
        BodyFormat::Wrapped => (WRAPPED_PREFIX, WRAPPED_SUFFIX),
        BodyFormat::WrappedBlox => (WRAPPED_PREFIX, WRAPPED_BLOX_SUFFIX),
        BodyFormat::PlainTx => (PLAIN_PREFIX, PLAIN_SUFFIX),
        BodyFormat::Batch => (BATCH_PREFIX, BATCH_SUFFIX),
        // the transaction is the body; there is nothing to wrap it in. The length prefix of
        // `LenPrefixedBinary` and the key prefix of `UdpRaw` are not constants, so they are
        // written in `build_body` instead.
        BodyFormat::Binary | BodyFormat::LenPrefixedBinary | BodyFormat::UdpRaw => (b"", b""),
    }
}

/// Lays out one launch's request body for this provider.
///
/// Three shapes, and the difference between them is most of the latency win in this module:
///   * base64 inside a JSON envelope — the transaction grows by a third and the hot path pays
///     an encode;
///   * raw bytes (blockrazor `/v2/sendBinaryTransaction`, 0slot `/txb`, astralane `/irisb`,
///     falcon `/binary`) — the transaction goes on the wire as-is;
///   * raw bytes behind a big-endian `u16` length (nozomi `/api/sendBatch`) — two extra bytes
///     for the path nozomi documents as their fastest, single transaction included.
///
/// `udp_prefix` is falcon's datagram key and is empty for every HTTP endpoint.
fn build_body(
    out: &mut Vec<u8>,
    format: BodyFormat,
    tx: &[u8],
    b64: &mut [u8],
    udp_prefix: &[u8],
    prefix: &'static [u8],
    suffix: &'static [u8],
) {
    out.clear();
    match format {
        BodyFormat::UdpRaw => {
            out.extend_from_slice(udp_prefix);
            out.extend_from_slice(tx);
        }
        BodyFormat::LenPrefixedBinary => {
            out.extend_from_slice(&(tx.len() as u16).to_be_bytes());
            out.extend_from_slice(tx);
        }
        BodyFormat::Binary => out.extend_from_slice(tx),
        _ => {
            let n = STANDARD
                .encode_slice(tx, b64)
                .expect("base64 buffer is large enough");
            out.extend_from_slice(prefix);
            out.extend_from_slice(&b64[..n]);
            out.extend_from_slice(suffix);
        }
    }
}

/// `Binary` posts the transaction bytes as-is, so it needs the octet-stream content type and
/// must skip the base64 step. Everything else is JSON.
fn content_type(format: BodyFormat) -> &'static [u8] {
    match format {
        BodyFormat::Binary | BodyFormat::LenPrefixedBinary | BodyFormat::UdpRaw => {
            b"application/octet-stream"
        }
        _ => b"application/json",
    }
}

/// Builds the request head for one endpoint. Everything that does not depend on the
/// transaction is computed once, at startup.
fn request_head(cfg: &ProviderConfig, host: &str, path: &str) -> Vec<u8> {
    let mut head = Vec::with_capacity(256);
    head.extend_from_slice(b"POST ");
    head.extend_from_slice(path.as_bytes());
    head.extend_from_slice(b" HTTP/1.1\r\nHost: ");
    head.extend_from_slice(host.as_bytes());
    head.extend_from_slice(b"\r\nContent-Type: ");
    head.extend_from_slice(content_type(cfg.body));
    head.extend_from_slice(b"\r\nConnection: keep-alive\r\n");
    for (k, v) in &cfg.headers {
        head.extend_from_slice(k.as_bytes());
        head.extend_from_slice(b": ");
        head.extend_from_slice(v.as_bytes());
        head.extend_from_slice(b"\r\n");
    }
    head
}

fn health_request(cfg: &ProviderConfig, host: &str) -> Vec<u8> {
    if cfg.health_path.is_empty() {
        return Vec::new();
    }
    let mut r = Vec::with_capacity(160);
    r.extend_from_slice(b"GET ");
    r.extend_from_slice(cfg.health_path.as_bytes());
    r.extend_from_slice(b" HTTP/1.1\r\nHost: ");
    r.extend_from_slice(host.as_bytes());
    r.extend_from_slice(b"\r\nConnection: keep-alive\r\n");
    for (k, v) in &cfg.headers {
        r.extend_from_slice(k.as_bytes());
        r.extend_from_slice(b": ");
        r.extend_from_slice(v.as_bytes());
        r.extend_from_slice(b"\r\n");
    }
    r.extend_from_slice(b"\r\n");
    r
}

fn write_usize(buf: &mut [u8; 20], mut v: usize) -> &[u8] {
    if v == 0 {
        buf[0] = b'0';
        return &buf[..1];
    }
    let mut i = buf.len();
    while v > 0 {
        i -= 1;
        buf[i] = b'0' + (v % 10) as u8;
        v /= 10;
    }
    &buf[i..]
}

/// Boot-time latency gate settings, from `SniperConfig`.
#[derive(Debug, Clone, Copy)]
pub struct LatencyGate {
    /// drop an endpoint slower than this, in milliseconds. 0 disables the gate.
    pub max_ms: u64,
    /// but never leave a provider with fewer than this many endpoints
    pub min_endpoints: usize,
}

impl Default for LatencyGate {
    fn default() -> Self {
        Self {
            max_ms: 20,
            min_endpoints: 2,
        }
    }
}

/// How many times each endpoint is timed at boot. The minimum of the samples is used: a
/// single round trip can be inflated by a cold cache or a scheduler hiccup, and one slow
/// sample must not retire an endpoint that is genuinely close.
const RTT_SAMPLES: usize = 3;

/// Drops endpoints slower than `max_endpoint_ms`, keeping at least `min_endpoints`.
///
/// Runs once, at startup, on the box that will actually be sending — which is the only place
/// the numbers mean anything. A distance table cannot substitute: the same hostname is a
/// different round trip from Frankfurt than from a laptop, and 0slot's Cloudflare-fronted
/// regions are slow for a reason no geography lookup would predict.
///
/// The fail-safe is the important half. A threshold that matches nothing would otherwise
/// leave a provider with no endpoints and the sniper would silently stop sending — the exact
/// class of quiet failure this module has been chasing. So the survivors are sorted by
/// measured latency and the fastest `min_endpoints` are kept regardless of the threshold,
/// which degrades a bad setting into "use the closest few" instead of "send nothing".
///
/// Endpoints that could not be measured at all (never connected, or a provider with no
/// health path, or UDP, which never answers) are kept. An unmeasurable endpoint is not
/// evidence of a slow one, and the keep-alive tick may well repair it later.
fn apply_latency_gate(
    endpoints: &mut Vec<Endpoint>,
    provider: &str,
    gate: LatencyGate,
    buf: &mut [u8],
) {
    let LatencyGate { max_ms, min_endpoints } = gate;
    if max_ms == 0 || endpoints.len() <= min_endpoints.max(1) {
        return;
    }

    let mut timed: Vec<(usize, Option<Duration>)> = Vec::with_capacity(endpoints.len());
    for (i, ep) in endpoints.iter_mut().enumerate() {
        let mut best: Option<Duration> = None;
        for _ in 0..RTT_SAMPLES {
            match ep.probe_rtt(buf) {
                Some(d) => best = Some(best.map_or(d, |b: Duration| b.min(d))),
                // a failed sample dropped the connection; stop poking at it
                None => break,
            }
        }
        timed.push((i, best));
    }

    let limit = Duration::from_millis(max_ms);
    // fastest first, unmeasured last (they are kept either way, but must not occupy the
    // "fastest N" slots that the fail-safe hands out)
    let mut ranked: Vec<&(usize, Option<Duration>)> = timed.iter().collect();
    ranked.sort_by_key(|(_, d)| d.unwrap_or(Duration::MAX));

    let mut keep = vec![false; endpoints.len()];
    let mut kept = 0usize;
    for (i, d) in ranked.iter() {
        let within = d.map_or(true, |d| d <= limit);
        if within || kept < min_endpoints {
            keep[*i] = true;
            kept += 1;
        }
    }

    for (i, d) in timed.iter() {
        if !keep[*i] {
            info!(
                "{}: dropping {} - {:.1}ms round trip, over the {}ms gate",
                provider,
                endpoints[*i].host,
                d.map_or(f64::NAN, |d| d.as_secs_f64() * 1000.0),
                max_ms
            );
        }
    }

    let forced: Vec<&str> = ranked
        .iter()
        .filter(|(i, d)| keep[*i] && d.is_some_and(|d| d > limit))
        .map(|(i, _)| endpoints[*i].host.as_str())
        .collect();
    if !forced.is_empty() {
        warn!(
            "{}: every endpoint is slower than the {}ms gate; keeping the {} fastest ({}) so \
             the provider still sends. Re-check SNIPER_MAX_ENDPOINT_MS on this box.",
            provider,
            max_ms,
            forced.len(),
            forced.join(", ")
        );
    }

    let mut i = 0;
    endpoints.retain(|_| {
        let k = keep[i];
        i += 1;
        k
    });
}

/// Spawns one sender thread for a provider and returns the handle the hot path pushes to.
pub fn spawn(
    cfg: ProviderConfig,
    signing_key: SigningKey,
    dry_run: bool,
    queue_depth: usize,
    spin_micros: u64,
    gate: LatencyGate,
    offsets: TipOffsets,
    tx_len: usize,
) -> Result<(ProviderHandle, JoinHandle<()>), String> {
    let tip_accounts = cfg.tip_account_bytes()?;
    let endpoints = cfg.endpoints();
    if endpoints.is_empty() {
        return Err(format!("provider {} has no endpoints", cfg.name));
    }
    // the sender patches by offset with no bounds check on the hot path, so prove here,
    // once, that every field it writes is inside the transaction
    for (what, end) in [
        ("tip_account", offsets.tip_account + 32),
        ("tip_lamports", offsets.tip_lamports + 8),
        ("cu_price", offsets.cu_price + 8),
    ] {
        if end > tx_len {
            return Err(format!(
                "provider {}: {what} offset runs past the {tx_len} byte template",
                cfg.name
            ));
        }
    }

    let (tx, rx): (Sender<Job>, Receiver<Job>) = crossbeam_channel::bounded(queue_depth);
    let metrics = Arc::new(SenderMetrics::default());
    let handle = ProviderHandle {
        name: cfg.name.clone(),
        tx,
        metrics: metrics.clone(),
        tip_accounts,
        tip_lamports: cfg.tip_lamports,
        cu_price: cfg.cu_price,
        endpoint_count: endpoints.len(),
    };

    let join = Builder::new()
        .name(format!("snipeTx_{}", cfg.name))
        .spawn(move || run(cfg, signing_key, dry_run, rx, metrics, spin_micros, gate, offsets))
        .expect("spawn sender thread");

    Ok((handle, join))
}

fn run(
    cfg: ProviderConfig,
    signing_key: SigningKey,
    dry_run: bool,
    rx: Receiver<Job>,
    metrics: Arc<SenderMetrics>,
    spin_micros: u64,
    gate: LatencyGate,
    offsets: TipOffsets,
) {
    let udp = cfg.body.is_udp();
    let udp_prefix = match cfg.udp_prefix_bytes() {
        Ok(p) => p,
        Err(e) => {
            warn!("{}: {e}; provider disabled", cfg.name);
            return;
        }
    };
    if udp && udp_prefix.is_empty() {
        warn!(
            "{}: UDP transport with no udp_prefix - every datagram would be unauthenticated \
             and silently dropped; provider disabled",
            cfg.name
        );
        return;
    }

    let mut endpoints = cfg
        .endpoints()
        .into_iter()
        .map(|(host, path)| Endpoint {
            head: request_head(&cfg, &host, &path),
            health: health_request(&cfg, &host),
            provider: cfg.name.clone(),
            host,
            port: cfg.port,
            tls: cfg.tls,
            udp,
            addr: None,
            conn: None,
            request: Vec::with_capacity(2560),
            awaiting: std::collections::VecDeque::new(),
            consecutive_rejects: 0,
        })
        .collect::<Vec<_>>();

    // Resolve every hostname once, here, before any launch can arrive. `connect` used to do
    // this inline on the hot path — see `Endpoint::resolve` and `CONNECT_TIMEOUT`.
    for ep in endpoints.iter_mut() {
        if let Err(e) = ep.resolve() {
            warn!("{}: cannot resolve {}: {e}", cfg.name, ep.host);
        }
    }

    // one submission for all of a provider's regions instead of one syscall each
    #[cfg(target_os = "linux")]
    let mut batch = if endpoints.len() > 1 {
        BatchWriter::new(endpoints.len())
    } else {
        None
    };
    #[cfg(target_os = "linux")]
    if batch.is_some() {
        info!(
            "{}: batching writes across {} endpoints with io_uring",
            cfg.name,
            endpoints.len()
        );
    }

    let (body_prefix, body_suffix) = body_wrappers(cfg.body);
    // this provider's own copy of the transaction: the shared body is patched into here so
    // the detect thread never has to produce one buffer per provider
    let mut tx = [0u8; MAX_TX];
    let mut b64 = vec![0u8; MAX_TX * 4 / 3 + 8];
    let mut body = Vec::with_capacity(2048);
    let mut drain = [0u8; 2048];

    // warm every connection before the first launch
    if !dry_run {
        for ep in endpoints.iter_mut() {
            match ep.connect() {
                Ok(()) => info!("{}: connected to {}:{}", cfg.name, ep.host, ep.port),
                Err(e) => warn!("{}: initial connect to {} failed: {e}", cfg.name, ep.host),
            }
        }
        // then retire the ones too far away to be worth a request, measured here rather
        // than assumed from a region name
        let before = endpoints.len();
        apply_latency_gate(&mut endpoints, &cfg.name, gate, &mut drain);
        if endpoints.len() != before {
            info!(
                "{}: latency gate kept {}/{} endpoints under {}ms",
                cfg.name,
                endpoints.len(),
                before,
                gate.max_ms
            );
        }
        // the gate leaves the sockets mid-conversation; start the launch path from a clean
        // slate so no stale reply can be attributed to a submit
        for ep in endpoints.iter_mut() {
            ep.awaiting.clear();
            drain_ready(ep, &mut drain, &metrics);
        }
    }

    // A receiver parked in `recv` has to be woken by the kernel, and that futex wake is
    // charged to the *detect* thread: measured at ~6.4us per provider, against ~150ns when
    // the receiver is already spinning. Spinning for a while after each job keeps bursts of
    // launches on the cheap path, and parking afterwards stops an idle sniper from burning a
    // core forever. `sender_spin_micros` in the config trades one against the other.
    let spin_window = Duration::from_micros(spin_micros);
    let mut spin_until = Instant::now() + spin_window;
    // The probe deadline is absolute, not "KEEPALIVE_SECS after the last thing happened".
    //
    // It used to be the latter, which quietly added the spin window to every gap: a launch
    // reset the spin, the spin ran for `sender_spin_micros` (2s in .env), and only then did
    // the 6s recv timeout start — so an endpoint could go 8s between probes. helius-sender
    // documents connection warming "when your application has gaps longer than 5 seconds"
    // and we measured its hard drop at 10s, which left almost no margin and made the safe
    // probe interval depend on an unrelated CPU-tuning knob. Now the two are independent.
    let mut next_probe = Instant::now() + Duration::from_secs(KEEPALIVE_SECS);

    loop {
        let now = Instant::now();
        let received = if spin_micros > 0 && now < spin_until && now < next_probe {
            match rx.try_recv() {
                Ok(job) => Ok(job),
                Err(TryRecvError::Empty) => {
                    std::hint::spin_loop();
                    continue;
                }
                Err(TryRecvError::Disconnected) => Err(RecvTimeoutError::Disconnected),
            }
        } else {
            rx.recv_timeout(next_probe.saturating_duration_since(now))
        };

        match received {
            Ok(job) => {
                spin_until = Instant::now() + spin_window;
                let len = job.base.len as usize;
                if len <= MSG_OFFSET || len > MAX_TX {
                    continue;
                }

                // take the shared body and stamp this provider's tip and fee on it. Doing it
                // here rather than on the detect thread is free: these threads are otherwise
                // idle and there is one per provider.
                tx[..len].copy_from_slice(&job.base.tx[..len]);
                let o = offsets;
                tx[o.tip_account..o.tip_account + 32].copy_from_slice(&job.tip_account);
                tx[o.tip_lamports..o.tip_lamports + 8]
                    .copy_from_slice(&job.tip_lamports.to_le_bytes());
                tx[o.cu_price..o.cu_price + 8].copy_from_slice(&job.cu_price.to_le_bytes());
                // release the shared body immediately so the detect thread's ring slot is
                // reusable on the next launch
                drop(job);

                // sign once for the whole provider, not once per region
                let sig = signing_key.sign(&tx[MSG_OFFSET..len]);
                tx[1..1 + 64].copy_from_slice(&sig.to_bytes());

                if dry_run {
                    metrics.sent.fetch_add(1, Ordering::Relaxed);
                    continue;
                }

                build_body(
                    &mut body,
                    cfg.body,
                    &tx[..len],
                    &mut b64,
                    &udp_prefix,
                    body_prefix,
                    body_suffix,
                );

                let mut num = [0u8; 20];
                let digits = write_usize(&mut num, body.len());

                // Build every request up front so a batched submit has all its buffers ready.
                //
                // A cold endpoint is SKIPPED, not reconnected. Reconnecting here is what the
                // old code did, and it put a DNS lookup plus a connect (five seconds, or a
                // TLS handshake on top) inline in the launch path, serially, ahead of the
                // write — so one blackholed region delayed every other region of the same
                // provider. The transaction is worthless by then, and the other providers'
                // threads are already sending. The dead endpoint is repaired on the next
                // keep-alive tick, off the critical path.
                for ep in endpoints.iter_mut() {
                    if ep.conn.is_none() {
                        metrics.skipped_cold.fetch_add(1, Ordering::Relaxed);
                        ep.request.clear();
                        continue;
                    }
                    ep.build_request(digits, &body);
                }

                // all of this provider's plain endpoints leave in one io_uring_enter, so the
                // last region is not eleven microseconds behind the first
                #[cfg(target_os = "linux")]
                if let Some(writer) = batch.as_mut() {
                    let items: Vec<(std::os::fd::RawFd, &[u8])> = endpoints
                        .iter()
                        .filter_map(|ep| ep.raw_fd().map(|fd| (fd, ep.request.as_slice())))
                        .collect();
                    if items.len() > 1 && items.len() <= writer.capacity() {
                        let lengths: Vec<usize> = items.iter().map(|(_, b)| b.len()).collect();
                        let results = writer.write_all(&items);
                        let mut idx = 0usize;
                        for ep in endpoints.iter_mut() {
                            if ep.raw_fd().is_none() {
                                continue;
                            }
                            match results.get(idx) {
                                Some(Ok(n)) if *n == lengths[idx] => {
                                    metrics.sent.fetch_add(1, Ordering::Relaxed);
                                    ep.expect_reply(false);
                                    ep.request.clear();
                                }
                                // a short write leaves the remainder for the loop below
                                Some(Ok(n)) => {
                                    ep.request.drain(..*n);
                                }
                                _ => ep.conn = None,
                            }
                            idx += 1;
                        }
                    }
                }

                // whatever the batch did not finish: TLS endpoints, short writes, and every
                // endpoint on a platform without io_uring.
                //
                // One attempt only. The old loop retried twice and reconnected between the
                // two, which is the same handshake-on-the-hot-path problem as above: a
                // provider whose socket has just died would burn two connects here while the
                // race it was queued for finishes without it. A failed write drops the
                // connection and the keep-alive tick rebuilds it.
                for ep in endpoints.iter_mut() {
                    if ep.request.is_empty() || ep.conn.is_none() {
                        continue;
                    }
                    let request = std::mem::take(&mut ep.request);
                    let outcome = ep.conn.as_mut().unwrap().write_all(&request);
                    ep.request = request;
                    match outcome {
                        Ok(()) => {
                            metrics.sent.fetch_add(1, Ordering::Relaxed);
                            ep.expect_reply(false);
                        }
                        Err(e) => {
                            debug!("{}: write to {} failed: {e}", cfg.name, ep.host);
                            ep.conn = None;
                            metrics.send_errors.fetch_add(1, Ordering::Relaxed);
                        }
                    }
                    ep.request.clear();
                }

                // clear whatever has already come back, without waiting for what has not.
                // this runs after the bytes are on the wire, so it is off the critical path,
                // and the previous launch's response is picked up here too
                for ep in endpoints.iter_mut() {
                    drain_ready(ep, &mut drain, &metrics);
                }
            }
            Err(RecvTimeoutError::Timeout) => {
                // keep-alive probe on every endpoint — see KEEPALIVE_SECS for the interval.
                //
                // The writes go out first and the replies are drained afterwards WITHOUT
                // blocking. An earlier version did a blocking read per endpoint, which meant
                // a provider with eleven regions could sit in this loop for over a second;
                // a launch arriving in that window waited behind it. Since the probe interval
                // had to come down to single digits (helius-sender hangs up at 10s), that
                // window would have been hit often. Nothing here needs the response — the
                // point is to put bytes on the socket — so it is send-all-then-drain, exactly
                // like the launch path above.
                next_probe = Instant::now() + Duration::from_secs(KEEPALIVE_SECS);
                if dry_run {
                    continue;
                }
                // This tick is now also where dead connections are repaired, since the launch
                // path no longer reconnects. Blocking here is fine — nothing is racing.
                for ep in endpoints.iter_mut() {
                    if ep.conn.is_none() {
                        // a name that failed to resolve at startup gets another chance; a
                        // cached address is reused without touching the resolver
                        if ep.connect().is_err() {
                            continue;
                        }
                        metrics.reconnects.fetch_add(1, Ordering::Relaxed);
                    }
                    // a UDP endpoint has nothing to keep warm: the socket above is all the
                    // state there is, and the provider would not answer a probe anyway
                    if ep.udp || ep.health.is_empty() {
                        continue;
                    }
                    let health = std::mem::take(&mut ep.health);
                    if let Some(c) = ep.conn.as_mut() {
                        if c.write_all(&health).is_err() {
                            ep.conn = None;
                        } else {
                            ep.expect_reply(true);
                        }
                    }
                    ep.health = health;
                }
                // pick up the replies, and notice any peer that hung up, without waiting
                for ep in endpoints.iter_mut() {
                    drain_ready(ep, &mut drain, &metrics);
                }
            }
            Err(RecvTimeoutError::Disconnected) => break,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cfg_with(hosts: Vec<&str>, body: BodyFormat) -> ProviderConfig {
        ProviderConfig {
            name: "t".into(),
            hosts: hosts.into_iter().map(String::from).collect(),
            host: String::new(),
            port: 80,
            path: "/?api-key=x".into(),
            tls: false,
            body,
            headers: vec![("api-key".into(), "y".into())],
            tip_accounts: vec!["11111111111111111111111111111111".into()],
            tip_account: String::new(),
            tip_lamports: 1,
            cu_price: 0,
            health_path: "/ping".into(),
            udp_prefix: String::new(),
            enabled: true,
        }
    }

    fn body_of(format: BodyFormat, tx: &[u8], udp_prefix: &[u8]) -> Vec<u8> {
        let (p, s) = body_wrappers(format);
        let mut out = Vec::new();
        let mut b64 = vec![0u8; MAX_TX * 4 / 3 + 8];
        build_body(&mut out, format, tx, &mut b64, udp_prefix, p, s);
        out
    }

    /// Nozomi's `/api/sendBatch` frames every transaction as `[u16 big-endian][bytes]`. Get
    /// the width or the endianness wrong and the provider reads a length that runs past the
    /// body — it answers 400 and the launch is simply gone, which is why `note_status` exists.
    #[test]
    fn len_prefixed_binary_frames_the_transaction_big_endian() {
        let tx = vec![0xABu8; 300];
        let body = body_of(BodyFormat::LenPrefixedBinary, &tx, b"");

        assert_eq!(body.len(), 302, "two byte prefix plus the transaction");
        assert_eq!(&body[..2], &[0x01, 0x2C], "300 as big-endian u16");
        assert_eq!(&body[2..], &tx[..]);
        // it must not have been base64'd on the way
        assert_eq!(
            content_type(BodyFormat::LenPrefixedBinary),
            b"application/octet-stream"
        );

        // and a full-size transaction still fits the u16 and nozomi's 66..=1232 range
        let big = vec![0u8; MAX_TX];
        let body = body_of(BodyFormat::LenPrefixedBinary, &big, b"");
        assert_eq!(&body[..2], &[0x04, 0xD0], "1232 as big-endian u16");
        assert_eq!(body.len(), MAX_TX + 2);
    }

    /// Falcon's datagram is `16-byte raw UUID || transaction` and nothing else. There is no
    /// reply on this transport, so a wrong prefix fails completely silently.
    #[test]
    fn udp_body_is_the_key_prefix_then_the_raw_transaction() {
        let tx = vec![0x11u8; 200];
        let key = [0xEFu8; 16];
        let body = body_of(BodyFormat::UdpRaw, &tx, &key);

        assert_eq!(body.len(), 216);
        assert_eq!(&body[..16], &key[..]);
        assert_eq!(&body[16..], &tx[..]);
        assert!(BodyFormat::UdpRaw.is_udp());
        assert!(!BodyFormat::Binary.is_udp());
    }

    /// A UDP endpoint has no request head, no Content-Length and no HTTP framing at all.
    #[test]
    fn udp_request_is_the_bare_datagram() {
        let mut cfg = cfg_with(vec!["a.example.com"], BodyFormat::UdpRaw);
        cfg.udp_prefix = "00112233445566778899aabbccddeeff".into();
        assert_eq!(cfg.udp_prefix_bytes().unwrap().len(), 16);

        let mut ep = Endpoint {
            provider: "t".into(),
            host: "a.example.com".into(),
            port: 9000,
            tls: false,
            udp: true,
            addr: None,
            head: request_head(&cfg, "a.example.com", &cfg.path),
            health: Vec::new(),
            conn: None,
            request: Vec::new(),
            awaiting: Default::default(),
            consecutive_rejects: 0,
        };
        ep.build_request(b"7", b"payload");
        assert_eq!(ep.request, b"payload");
    }

    /// A malformed prefix must be an error, not an empty one: an unauthenticated datagram is
    /// dropped in silence and looks identical to a successful send.
    #[test]
    fn a_bad_udp_prefix_is_rejected_rather_than_silently_empty() {
        let mut cfg = cfg_with(vec!["a.example.com"], BodyFormat::UdpRaw);
        cfg.udp_prefix = "abc".into();
        assert!(cfg.udp_prefix_bytes().is_err(), "odd digit count");
        cfg.udp_prefix = "zz".into();
        assert!(cfg.udp_prefix_bytes().is_err(), "not hex");
        cfg.udp_prefix = String::new();
        assert_eq!(cfg.udp_prefix_bytes().unwrap(), Vec::<u8>::new());
    }

    /// Keep-alive replies must never be counted as rejections: jito 404s every path and
    /// nextblock's `/health` 401s for our key, both deliberately. Only submits count.
    #[test]
    fn only_submit_replies_are_counted_as_rejections() {
        let metrics = SenderMetrics::default();
        let mut ep = Endpoint {
            provider: "t".into(),
            host: "a.example.com".into(),
            port: 80,
            tls: false,
            udp: false,
            addr: None,
            head: Vec::new(),
            health: Vec::new(),
            conn: None,
            request: Vec::new(),
            awaiting: Default::default(),
            consecutive_rejects: 0,
        };

        let feed = |ep: &mut Endpoint, raw: &[u8]| {
            let mut codes = StatusCodes::default();
            codes.scan(raw);
            note_status(&codes, ep, &metrics);
        };

        // a probe 404 is expected and must not register
        ep.expect_reply(true);
        feed(&mut ep, b"HTTP/1.1 404 Not Found\r\n\r\n");
        assert_eq!(metrics.rejected.load(Ordering::Relaxed), 0);

        // a submit 401 is a dead key and must
        ep.expect_reply(false);
        feed(&mut ep, b"HTTP/1.1 401 Unauthorized\r\n\r\n");
        assert_eq!(metrics.rejected.load(Ordering::Relaxed), 1);
        assert_eq!(ep.consecutive_rejects, 1);

        // a success clears the streak
        ep.expect_reply(false);
        feed(&mut ep, b"HTTP/1.1 200 OK\r\n\r\n");
        assert_eq!(metrics.rejected.load(Ordering::Relaxed), 1);
        assert_eq!(ep.consecutive_rejects, 0);

        // pipelined replies are attributed in request order: the 404 belongs to the probe
        // and is ignored, the 429 belongs to the submit and counts
        ep.expect_reply(true);
        ep.expect_reply(false);
        feed(
            &mut ep,
            b"HTTP/1.1 404 Not Found\r\n\r\nHTTP/1.1 429 Too Many Requests\r\n\r\n",
        );
        assert_eq!(metrics.rejected.load(Ordering::Relaxed), 2);
        assert!(ep.awaiting.is_empty(), "every reply was attributed");
    }

    fn ep_named(host: &str) -> Endpoint {
        Endpoint {
            provider: "t".into(),
            host: host.into(),
            port: 80,
            tls: false,
            udp: false,
            addr: None,
            head: Vec::new(),
            health: Vec::new(),
            conn: None,
            request: Vec::new(),
            awaiting: Default::default(),
            consecutive_rejects: 0,
        }
    }

    /// The gate must never be able to leave a provider unable to send. An endpoint that could
    /// not be measured at all (no connection, no health path, UDP) is not evidence of a slow
    /// endpoint, so it is kept.
    #[test]
    fn latency_gate_keeps_unmeasurable_endpoints() {
        let mut eps: Vec<Endpoint> = ["a", "b", "c", "d"].iter().map(|h| ep_named(h)).collect();
        let mut buf = [0u8; 256];
        // none of these has a connection or a health request, so every probe returns None
        apply_latency_gate(
            &mut eps,
            "t",
            LatencyGate {
                max_ms: 1,
                min_endpoints: 2,
            },
            &mut buf,
        );
        assert_eq!(eps.len(), 4, "unmeasurable endpoints must not be dropped");
    }

    /// A gate wider than the endpoint count, or a disabled one, is a no-op.
    #[test]
    fn latency_gate_is_a_noop_when_disabled_or_too_small_to_matter() {
        let mut buf = [0u8; 256];

        let mut eps: Vec<Endpoint> = ["a", "b", "c"].iter().map(|h| ep_named(h)).collect();
        apply_latency_gate(
            &mut eps,
            "t",
            LatencyGate {
                max_ms: 0,
                min_endpoints: 1,
            },
            &mut buf,
        );
        assert_eq!(eps.len(), 3, "max_ms 0 disables the gate");

        let mut eps: Vec<Endpoint> = ["a", "b"].iter().map(|h| ep_named(h)).collect();
        apply_latency_gate(
            &mut eps,
            "t",
            LatencyGate {
                max_ms: 1,
                min_endpoints: 5,
            },
            &mut buf,
        );
        assert_eq!(eps.len(), 2, "cannot drop below min_endpoints");
    }

    /// The default has to be safe to ship: a gate that keeps nothing would stop the sniper
    /// sending at all, silently.
    #[test]
    fn default_gate_always_leaves_something_to_send() {
        let g = LatencyGate::default();
        assert_eq!(g.max_ms, 20);
        assert!(g.min_endpoints >= 1, "a provider with no endpoints cannot send");
    }

    /// `awaiting` grows on every write and shrinks on every reply, so a peer that stops
    /// answering must not be able to grow it without bound.
    #[test]
    fn awaiting_queue_is_bounded() {
        let mut ep = Endpoint {
            provider: "t".into(),
            host: "a.example.com".into(),
            port: 80,
            tls: false,
            udp: false,
            addr: None,
            head: Vec::new(),
            health: Vec::new(),
            conn: None,
            request: Vec::new(),
            awaiting: Default::default(),
            consecutive_rejects: 0,
        };
        for _ in 0..MAX_AWAITING * 4 {
            ep.expect_reply(false);
        }
        assert!(ep.awaiting.len() <= MAX_AWAITING);
    }

    #[test]
    fn writes_decimal_lengths() {
        let mut b = [0u8; 20];
        assert_eq!(write_usize(&mut b, 0), b"0");
        assert_eq!(write_usize(&mut b, 7), b"7");
        assert_eq!(write_usize(&mut b, 1234), b"1234");
    }

    #[test]
    fn head_carries_host_and_custom_headers() {
        let cfg = cfg_with(vec!["a.example.com", "b.example.com"], BodyFormat::JsonRpc);
        let head = String::from_utf8(request_head(&cfg, "b.example.com", &cfg.path)).unwrap();
        assert!(head.starts_with("POST /?api-key=x HTTP/1.1\r\n"));
        assert!(head.contains("Host: b.example.com"));
        assert!(head.contains("api-key: y"));
        assert!(!head.contains("Content-Length"));
    }

    #[test]
    fn every_body_format_wraps_the_base64_transaction() {
        let b64 = "QUJD";
        for (format, expect) in [
            (BodyFormat::JsonRpc, r#""method":"sendTransaction""#),
            (BodyFormat::Wrapped, r#"{"transaction":{"content":"QUJD"}"#),
            (BodyFormat::WrappedBlox, r#"{"transaction":{"content":"QUJD"}"#),
            (BodyFormat::PlainTx, r#"{"transaction":"QUJD"}"#),
            (BodyFormat::Batch, r#"{"transactions":["QUJD"]}"#),
        ] {
            let (p, s) = body_wrappers(format);
            let body = [p, b64.as_bytes(), s].concat();
            let body = String::from_utf8(body).unwrap();
            assert!(body.contains(expect), "{format:?} produced {body}");
            assert!(body.contains(b64));
        }
    }

    /// bloXroute holds a transaction for up to four slots under its default SP_MEDIUM, and
    /// nextblock shares the same wrapper, so the two must not collapse back into one format.
    /// `"low"` is rejected by their parser — only the enum spelling works.
    #[test]
    fn bloxroute_body_disables_the_leader_risk_hold() {
        let (_, blox) = body_wrappers(BodyFormat::WrappedBlox);
        let blox = std::str::from_utf8(blox).unwrap();
        assert!(blox.contains(r#""submitProtection":"SP_LOW""#), "{blox}");
        assert!(blox.contains(r#""frontRunningProtection":false"#), "{blox}");

        let (_, plain) = body_wrappers(BodyFormat::Wrapped);
        let plain = std::str::from_utf8(plain).unwrap();
        assert!(!plain.contains("submitProtection"), "nextblock must not get it: {plain}");
        assert!(!plain.contains("useStakedRPCs"), "nextblock must not get it: {plain}");
    }

    /// Without `useStakedRPCs`, SP_LOW is documented as "direct submission to Jito" — i.e.
    /// bloxroute becomes a slower path to a block engine we already hit ourselves. The flag
    /// is what buys weighted-stake QoS submission to the leader, and it has exactly two
    /// documented preconditions, both of which this body must keep satisfying.
    #[test]
    fn bloxroute_body_asks_for_staked_submission() {
        let (_, blox) = body_wrappers(BodyFormat::WrappedBlox);
        let blox = std::str::from_utf8(blox).unwrap();
        assert!(blox.contains(r#""useStakedRPCs":true"#), "{blox}");
        // precondition 1: bloXroute rejects useStakedRPCs unless front-running protection is
        // off. (Precondition 2 is the >=0.001 SOL tip, which lives in the catalogue's
        // min_tip and is floored again per provider in lib.rs.)
        assert!(blox.contains(r#""frontRunningProtection":false"#), "{blox}");
        // the body must stay valid JSON: one object, no doubled separators
        assert!(blox.ends_with('}') && !blox.contains(",,"), "{blox}");
    }

    /// Binary submission is the whole reason the format exists: the transaction bytes go on
    /// the wire untouched, so there must be nothing wrapped around them and the content type
    /// has to say octet-stream or blockrazor will try to parse them as JSON.
    #[test]
    fn binary_body_is_the_raw_transaction() {
        let (prefix, suffix) = body_wrappers(BodyFormat::Binary);
        assert!(prefix.is_empty() && suffix.is_empty());
        assert_eq!(content_type(BodyFormat::Binary), b"application/octet-stream");
        assert_eq!(content_type(BodyFormat::JsonRpc), b"application/json");

        let cfg = cfg_with(vec!["a.example.com"], BodyFormat::Binary);
        let head = String::from_utf8(request_head(&cfg, "a.example.com", &cfg.path)).unwrap();
        assert!(head.contains("Content-Type: application/octet-stream"), "{head}");
        assert!(!head.contains("application/json"), "{head}");
    }

    #[test]
    fn endpoints_expand_to_every_host() {
        let cfg = cfg_with(vec!["a.example.com", "b.example.com"], BodyFormat::JsonRpc);
        let eps = cfg.endpoints();
        assert_eq!(eps.len(), 2);
        assert_eq!(eps[0].0, "a.example.com");
        assert_eq!(eps[1].0, "b.example.com");
    }

    /// A host may carry its own path, which is how providers that publish per-region paths
    /// (blockrazor's `/sendTransaction`) coexist with ones that do not.
    #[test]
    fn host_can_override_the_path() {
        let mut cfg = cfg_with(vec!["a.example.com/sendTransaction"], BodyFormat::PlainTx);
        cfg.path = "/".into();
        let eps = cfg.endpoints();
        assert_eq!(eps[0].0, "a.example.com");
        assert_eq!(eps[0].1, "/sendTransaction");
    }
}
