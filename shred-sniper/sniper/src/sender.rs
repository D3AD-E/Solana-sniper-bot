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
    pub sent: AtomicU64,
    pub send_errors: AtomicU64,
    pub reconnects: AtomicU64,
    pub dropped_full: AtomicU64,
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
}

impl Conn {
    fn write_all(&mut self, buf: &[u8]) -> std::io::Result<()> {
        match self {
            Conn::Plain(s) => s.write_all(buf).and_then(|_| s.flush()),
            Conn::Tls(s) => s.write_all(buf).and_then(|_| s.flush()),
        }
    }
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        match self {
            Conn::Plain(s) => s.read(buf),
            Conn::Tls(s) => s.read(buf),
        }
    }
    fn set_nonblocking(&self, on: bool) -> std::io::Result<()> {
        match self {
            Conn::Plain(s) => s.set_nonblocking(on),
            Conn::Tls(s) => s.sock.set_nonblocking(on),
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
fn drain_ready(ep: &mut Endpoint, buf: &mut [u8]) {
    let Some(conn) = ep.conn.as_mut() else { return };
    if conn.set_nonblocking(true).is_err() {
        ep.conn = None;
        return;
    }
    let mut dead = false;
    loop {
        match conn.read(buf) {
            // a keep-alive connection only reports 0 bytes when the peer closed it
            Ok(0) => {
                dead = true;
                break;
            }
            // a full buffer means there may be more behind it
            Ok(n) if n == buf.len() => continue,
            Ok(_) => break,
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
    host: String,
    port: u16,
    tls: bool,
    /// everything up to Content-Length, precomputed
    head: Vec<u8>,
    health: Vec<u8>,
    conn: Option<Conn>,
    /// this endpoint's fully built request, kept alive across a batched submit
    request: Vec<u8>,
}

impl Endpoint {
    /// Plain TCP endpoints can go through the batch writer; TLS cannot, because rustls owns
    /// the record framing.
    #[cfg(target_os = "linux")]
    fn raw_fd(&self) -> Option<std::os::fd::RawFd> {
        use std::os::fd::AsRawFd;
        match self.conn.as_ref()? {
            Conn::Plain(s) => Some(s.as_raw_fd()),
            Conn::Tls(_) => None,
        }
    }

    fn build_request(&mut self, length_digits: &[u8], body: &[u8]) {
        self.request.clear();
        self.request.extend_from_slice(&self.head);
        self.request.extend_from_slice(b"Content-Length: ");
        self.request.extend_from_slice(length_digits);
        self.request.extend_from_slice(b"\r\n\r\n");
        self.request.extend_from_slice(body);
    }
}

impl Endpoint {
    fn connect(&mut self) -> std::io::Result<()> {
        let addr = (self.host.as_str(), self.port)
            .to_socket_addrs()?
            .next()
            .ok_or_else(|| std::io::Error::new(std::io::ErrorKind::NotFound, "no address"))?;
        let tcp = TcpStream::connect_timeout(&addr, Duration::from_secs(5))?;
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
        Ok(())
    }
}

const JSON_RPC_PREFIX: &[u8] =
    br#"{"jsonrpc":"2.0","id":1,"method":"sendTransaction","params":[""#;
const JSON_RPC_SUFFIX: &[u8] = br#"",{"encoding":"base64","skipPreflight":true,"maxRetries":0}]}"#;

// nextblock / bloxroute
const WRAPPED_PREFIX: &[u8] = br#"{"transaction":{"content":""#;
const WRAPPED_SUFFIX: &[u8] = br#""},"skipPreFlight":true,"frontRunningProtection":false}"#;

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
        BodyFormat::PlainTx => (PLAIN_PREFIX, PLAIN_SUFFIX),
        BodyFormat::Batch => (BATCH_PREFIX, BATCH_SUFFIX),
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
    head.extend_from_slice(b"\r\nContent-Type: application/json\r\nConnection: keep-alive\r\n");
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

/// Spawns one sender thread for a provider and returns the handle the hot path pushes to.
pub fn spawn(
    cfg: ProviderConfig,
    signing_key: SigningKey,
    dry_run: bool,
    queue_depth: usize,
    spin_micros: u64,
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
        .spawn(move || run(cfg, signing_key, dry_run, rx, metrics, spin_micros, offsets))
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
    offsets: TipOffsets,
) {
    let mut endpoints = cfg
        .endpoints()
        .into_iter()
        .map(|(host, path)| Endpoint {
            head: request_head(&cfg, &host, &path),
            health: health_request(&cfg, &host),
            host,
            port: cfg.port,
            tls: cfg.tls,
            conn: None,
            request: Vec::with_capacity(2560),
        })
        .collect::<Vec<_>>();

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
    }

    // A receiver parked in `recv` has to be woken by the kernel, and that futex wake is
    // charged to the *detect* thread: measured at ~6.4us per provider, against ~150ns when
    // the receiver is already spinning. Spinning for a while after each job keeps bursts of
    // launches on the cheap path, and parking afterwards stops an idle sniper from burning a
    // core forever. `sender_spin_micros` in the config trades one against the other.
    let spin_window = Duration::from_micros(spin_micros);
    let mut spin_until = Instant::now() + spin_window;

    loop {
        let received = if spin_micros > 0 && Instant::now() < spin_until {
            match rx.try_recv() {
                Ok(job) => Ok(job),
                Err(TryRecvError::Empty) => {
                    std::hint::spin_loop();
                    continue;
                }
                Err(TryRecvError::Disconnected) => Err(RecvTimeoutError::Disconnected),
            }
        } else {
            rx.recv_timeout(Duration::from_secs(50))
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

                let n = STANDARD
                    .encode_slice(&tx[..len], &mut b64)
                    .expect("base64 buffer is large enough");
                body.clear();
                body.extend_from_slice(body_prefix);
                body.extend_from_slice(&b64[..n]);
                body.extend_from_slice(body_suffix);

                let mut num = [0u8; 20];
                let digits = write_usize(&mut num, body.len());

                // reconnect what dropped, then build every request up front so a batched
                // submit has all its buffers ready
                for ep in endpoints.iter_mut() {
                    if ep.conn.is_none() && ep.connect().is_ok() {
                        metrics.reconnects.fetch_add(1, Ordering::Relaxed);
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

                // whatever the batch did not finish: TLS endpoints, short writes, reconnects
                for ep in endpoints.iter_mut() {
                    if ep.request.is_empty() {
                        continue;
                    }
                    let mut ok = false;
                    for _ in 0..2 {
                        if ep.conn.is_none() {
                            if ep.connect().is_err() {
                                break;
                            }
                            metrics.reconnects.fetch_add(1, Ordering::Relaxed);
                            ep.build_request(digits, &body);
                        }
                        let request = std::mem::take(&mut ep.request);
                        let outcome = ep.conn.as_mut().unwrap().write_all(&request);
                        ep.request = request;
                        match outcome {
                            Ok(()) => {
                                ok = true;
                                ep.request.clear();
                                break;
                            }
                            Err(e) => {
                                debug!("{}: write to {} failed: {e}", cfg.name, ep.host);
                                ep.conn = None;
                            }
                        }
                    }
                    if ok {
                        metrics.sent.fetch_add(1, Ordering::Relaxed);
                    } else {
                        metrics.send_errors.fetch_add(1, Ordering::Relaxed);
                        ep.request.clear();
                    }
                }

                // clear whatever has already come back, without waiting for what has not.
                // this runs after the bytes are on the wire, so it is off the critical path,
                // and the previous launch's response is picked up here too
                for ep in endpoints.iter_mut() {
                    drain_ready(ep, &mut drain);
                }
            }
            Err(RecvTimeoutError::Timeout) => {
                // 50s keep-alive probe on every endpoint, matching what the providers expect
                if dry_run {
                    continue;
                }
                for ep in endpoints.iter_mut() {
                    if ep.health.is_empty() {
                        continue;
                    }
                    if ep.conn.is_none() && ep.connect().is_err() {
                        continue;
                    }
                    let health = ep.health.clone();
                    if let Some(c) = ep.conn.as_mut() {
                        if c.write_all(&health).is_err() || c.read(&mut drain).is_err() {
                            ep.conn = None;
                        }
                    }
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
            enabled: true,
        }
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
