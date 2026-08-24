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
    config::{BodyFormat, ProviderConfig},
    template::MSG_OFFSET,
};

/// Max serialized transaction size on Solana.
pub const MAX_TX: usize = 1232;

/// A transaction that is fully patched but not yet signed.
#[derive(Clone)]
pub struct Job {
    pub len: u16,
    pub tx: [u8; MAX_TX],
}

impl Default for Job {
    fn default() -> Self {
        Self {
            len: 0,
            tx: [0u8; MAX_TX],
        }
    }
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
) -> Result<(ProviderHandle, JoinHandle<()>), String> {
    let tip_accounts = cfg.tip_account_bytes()?;
    let endpoints = cfg.endpoints();
    if endpoints.is_empty() {
        return Err(format!("provider {} has no endpoints", cfg.name));
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
        .spawn(move || run(cfg, signing_key, dry_run, rx, metrics, spin_micros))
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
        })
        .collect::<Vec<_>>();

    let (body_prefix, body_suffix) = body_wrappers(cfg.body);
    let mut b64 = vec![0u8; MAX_TX * 4 / 3 + 8];
    let mut body = Vec::with_capacity(2048);
    let mut request = Vec::with_capacity(2560);
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
            Ok(mut job) => {
                spin_until = Instant::now() + spin_window;
                let len = job.len as usize;
                if len <= MSG_OFFSET || len > MAX_TX {
                    continue;
                }

                // sign once for the whole provider, not once per region
                let sig = signing_key.sign(&job.tx[MSG_OFFSET..len]);
                job.tx[1..1 + 64].copy_from_slice(&sig.to_bytes());

                if dry_run {
                    metrics.sent.fetch_add(1, Ordering::Relaxed);
                    continue;
                }

                let n = STANDARD
                    .encode_slice(&job.tx[..len], &mut b64)
                    .expect("base64 buffer is large enough");
                body.clear();
                body.extend_from_slice(body_prefix);
                body.extend_from_slice(&b64[..n]);
                body.extend_from_slice(body_suffix);

                let mut num = [0u8; 20];
                let digits = write_usize(&mut num, body.len());

                for ep in endpoints.iter_mut() {
                    request.clear();
                    request.extend_from_slice(&ep.head);
                    request.extend_from_slice(b"Content-Length: ");
                    request.extend_from_slice(digits);
                    request.extend_from_slice(b"\r\n\r\n");
                    request.extend_from_slice(&body);

                    let mut ok = false;
                    for _ in 0..2 {
                        if ep.conn.is_none() {
                            if ep.connect().is_err() {
                                break;
                            }
                            metrics.reconnects.fetch_add(1, Ordering::Relaxed);
                        }
                        match ep.conn.as_mut().unwrap().write_all(&request) {
                            Ok(()) => {
                                ok = true;
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
                    }
                }

                // drain responses so the connections stay usable for the next launch
                for ep in endpoints.iter_mut() {
                    if let Some(c) = ep.conn.as_mut() {
                        if c.read(&mut drain).is_err() {
                            ep.conn = None;
                        }
                    }
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
