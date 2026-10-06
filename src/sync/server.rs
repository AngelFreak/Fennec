//! The receiver: a TLS listener on its own thread, a thread per connection,
//! and the protocol's routes.

use std::net::{IpAddr, SocketAddr, TcpListener, TcpStream, UdpSocket};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use serde::Deserialize;
use serde_json::json;

use super::discovery::Discovery;
use super::http::{ReadError, Request, Response, read_request, write_response};
use super::inbound::{self, Target, valid_uuid};
use super::pairing::{Pairing, PairingOffer, Refusal, pairing_code};
use super::tls::Identity;
use super::{EventSink, PROTOCOL_VERSION, SyncEvent, desktop_name, random_bytes, secret_hash};
use crate::config::{Paths, Settings};
use crate::store::Store;

/// Largest request body: a chunk (the phone sends 1 MB) or JSON.
const MAX_BODY: usize = 4 * 1024 * 1024;
const MAX_CONNECTIONS: usize = 16;
const IO_TIMEOUT: Duration = Duration::from_secs(30);
/// Uploads that stop arriving are deleted after this long.
const STALE_AFTER_MS: i64 = 7 * 24 * 60 * 60 * 1000;
const SWEEP_EVERY: Duration = Duration::from_secs(60 * 60);

pub struct ReceiverConfig {
    pub bind: SocketAddr,
    pub paths: Paths,
    pub default_project: Option<i64>,
    pub default_template: String,
    /// Announce Fennec with mDNS.
    pub advertise: bool,
    /// How long a phone waits for Allow before pairing fails.
    pub pair_wait: Duration,
}

impl ReceiverConfig {
    /// Every network interface on the port from Settings.
    pub fn from_settings(paths: &Paths, settings: &Settings) -> Self {
        Self {
            bind: SocketAddr::from(([0, 0, 0, 0], settings.phone.port)),
            paths: paths.clone(),
            default_project: settings.phone.default_project,
            default_template: settings.default_template.clone(),
            advertise: true,
            pair_wait: Duration::from_secs(120),
        }
    }
}

struct Shared {
    db: PathBuf,
    templates: PathBuf,
    target: Mutex<Target>,
    /// Uploads touch files and rows together; one at a time.
    uploads: Mutex<()>,
    pairing: Mutex<Pairing>,
    events: EventSink,
    pin: String,
    name: String,
    pair_wait: Duration,
}

/// A running receiver. Stops when dropped.
pub struct Receiver {
    shared: Arc<Shared>,
    stop: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
    local_addr: SocketAddr,
    _discovery: Option<Discovery>,
}

impl Receiver {
    pub fn start(config: ReceiverConfig, events: EventSink) -> Result<Self, String> {
        let sync_dir = config.paths.data_dir.join("sync");
        let identity = Identity::load_or_create(&sync_dir)?;
        let tls = identity.server_config()?;
        let listener = TcpListener::bind(config.bind).map_err(|e| match e.kind() {
            std::io::ErrorKind::AddrInUse => format!(
                "Port {} is in use by another program. Choose another port.",
                config.bind.port()
            ),
            _ => format!("Could not listen on port {}: {e}", config.bind.port()),
        })?;
        listener.set_nonblocking(true).map_err(|e| e.to_string())?;
        let local_addr = listener.local_addr().map_err(|e| e.to_string())?;

        let name = desktop_name();
        let shared = Arc::new(Shared {
            db: config.paths.database(),
            templates: config.paths.templates(),
            target: Mutex::new(Target {
                incoming: sync_dir.join("incoming"),
                audio: config.paths.audio(),
                default_project: config.default_project,
                default_template: config.default_template,
            }),
            uploads: Mutex::new(()),
            pairing: Mutex::new(Pairing::default()),
            events,
            pin: identity.pin.clone(),
            name: name.clone(),
            pair_wait: config.pair_wait,
        });
        let discovery = config
            .advertise
            .then(|| Discovery::advertise(&name, local_addr.port(), &identity.pin[..12]))
            .and_then(|r| {
                r.map_err(|e| tracing::warn!("phones cannot find Fennec by name: {e}"))
                    .ok()
            });

        let stop = Arc::new(AtomicBool::new(false));
        let thread = {
            let (shared, stop) = (Arc::clone(&shared), Arc::clone(&stop));
            std::thread::Builder::new()
                .name("fennec-phone".into())
                .spawn(move || accept_loop(listener, tls, shared, stop))
                .map_err(|e| e.to_string())?
        };
        tracing::info!("receiving recordings from phones on {local_addr}");
        Ok(Self {
            shared,
            stop,
            thread: Some(thread),
            local_addr,
            _discovery: discovery,
        })
    }

    pub fn local_addr(&self) -> SocketAddr {
        self.local_addr
    }

    /// The certificate pin phones check.
    pub fn pin(&self) -> &str {
        &self.shared.pin
    }

    /// Opens a pairing offer (replacing an earlier one) for the QR code.
    pub fn offer_pairing(&self) -> PairingOffer {
        let (token, expires_at) = self.shared.pairing.lock().unwrap().open(Instant::now());
        let ip = match self.local_addr.ip() {
            ip if ip.is_unspecified() => lan_ip().unwrap_or(IpAddr::from([127, 0, 0, 1])),
            ip => ip,
        };
        PairingOffer {
            address: SocketAddr::new(ip, self.local_addr.port()).to_string(),
            token,
            pin: self.shared.pin.clone(),
            name: self.shared.name.clone(),
            expires_at,
        }
    }

    /// Closes the pairing offer; a phone waiting for Allow is denied.
    pub fn cancel_pairing(&self) {
        self.shared.pairing.lock().unwrap().close();
    }

    /// Allow or Deny for a [`SyncEvent::PairRequest`].
    pub fn decide(&self, request: u64, allow: bool) {
        self.shared.pairing.lock().unwrap().decide(request, allow);
    }

    /// Project and template for recordings that name none.
    pub fn set_defaults(&self, default_project: Option<i64>, default_template: &str) {
        let mut t = self.shared.target.lock().unwrap();
        t.default_project = default_project;
        t.default_template = default_template.to_string();
    }
}

impl Drop for Receiver {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        self.shared.pairing.lock().unwrap().close();
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

/// The address phones on the network reach this computer at: the one on
/// the default route. Sends nothing.
fn lan_ip() -> Option<IpAddr> {
    let s = UdpSocket::bind("0.0.0.0:0").ok()?;
    s.connect("192.0.2.1:9").ok()?;
    s.local_addr()
        .ok()
        .map(|a| a.ip())
        .filter(|ip| !ip.is_unspecified())
}

fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

fn sweep(shared: &Shared) {
    let target = shared.target.lock().unwrap().clone();
    match Store::open(&shared.db) {
        Ok(store) => {
            let n = inbound::sweep_stale(&store, &target, now_ms(), STALE_AFTER_MS);
            if n > 0 {
                tracing::info!("deleted {n} unfinished uploads from phones");
            }
        }
        Err(e) => tracing::warn!("checking for unfinished uploads: {e}"),
    }
}

fn accept_loop(
    listener: TcpListener,
    tls: Arc<rustls::ServerConfig>,
    shared: Arc<Shared>,
    stop: Arc<AtomicBool>,
) {
    let active = Arc::new(AtomicUsize::new(0));
    let mut last_sweep: Option<Instant> = None;
    while !stop.load(Ordering::Relaxed) {
        if last_sweep.is_none_or(|t| t.elapsed() > SWEEP_EVERY) {
            sweep(&shared);
            last_sweep = Some(Instant::now());
        }
        match listener.accept() {
            Ok((tcp, peer)) => {
                if active.load(Ordering::Relaxed) >= MAX_CONNECTIONS {
                    tracing::warn!("too many phone connections; dropping {peer}");
                    continue;
                }
                active.fetch_add(1, Ordering::Relaxed);
                let (tls, shared, active) = (Arc::clone(&tls), Arc::clone(&shared), Arc::clone(&active));
                let spawned = std::thread::Builder::new()
                    .name("fennec-phone-conn".into())
                    .spawn(move || {
                        serve(&shared, tls, tcp);
                        active.fetch_sub(1, Ordering::Relaxed);
                    });
                if let Err(e) = spawned {
                    tracing::warn!("serving {peer}: {e}");
                }
            }
            Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                std::thread::sleep(Duration::from_millis(50));
            }
            Err(e) => {
                tracing::warn!("accepting a phone connection: {e}");
                std::thread::sleep(Duration::from_millis(200));
            }
        }
    }
}

fn serve(shared: &Shared, tls: Arc<rustls::ServerConfig>, tcp: TcpStream) {
    let setup = tcp
        .set_nonblocking(false)
        .and_then(|()| tcp.set_read_timeout(Some(IO_TIMEOUT)))
        .and_then(|()| tcp.set_write_timeout(Some(IO_TIMEOUT)));
    if setup.is_err() {
        return;
    }
    let Ok(conn) = rustls::ServerConnection::new(tls) else {
        return;
    };
    let mut stream = rustls::StreamOwned::new(conn, tcp);
    let response = match read_request(&mut stream, MAX_BODY) {
        Ok(req) => route(shared, &req),
        // Includes failed handshakes, e.g. a phone that pinned another Fennec.
        Err(ReadError::Io(e)) => {
            tracing::debug!("phone connection closed: {e}");
            return;
        }
        Err(ReadError::Bad(m)) => Response::error(400, "bad_request", m),
        Err(ReadError::TooLarge) => Response::error(413, "too_large", "Send chunks of at most 4 MB."),
    };
    if write_response(&mut stream, &response).is_ok() {
        stream.conn.send_close_notify();
        let _ = std::io::Write::flush(&mut stream);
    }
}

fn route(shared: &Shared, req: &Request) -> Response {
    let segs: Vec<&str> = req.segments.iter().map(String::as_str).collect();
    let method = req.method.as_str();
    if let ("POST", ["v1", "pair"]) = (method, segs.as_slice()) {
        return pair(shared, req);
    }

    let store = match Store::open(&shared.db) {
        Ok(s) => s,
        Err(e) => {
            tracing::error!("phone request: {e}");
            return Response::error(503, "unavailable", "Fennec cannot open its database.");
        }
    };
    let device = req
        .bearer()
        .and_then(|secret| store.device_by_secret_hash(&secret_hash(secret)).ok().flatten());
    let Some(device) = device else {
        return Response::error(
            401,
            "not_paired",
            "This phone is not paired with Fennec. Pair it again.",
        );
    };
    let target = shared.target.lock().unwrap().clone();
    match (method, segs.as_slice()) {
        ("GET", ["v1", "info"]) => info(shared, &store),
        ("GET", ["v1", "recordings"]) => {
            inbound::statuses(&store, &target, device, req.query("ids").unwrap_or(""))
        }
        ("PUT", ["v1", "recordings", id]) if valid_uuid(id) => {
            let _one = shared.uploads.lock().unwrap();
            inbound::announce(&store, &target, device, id, &req.body)
        }
        ("PUT", ["v1", "recordings", id, "audio"]) if valid_uuid(id) => {
            let _one = shared.uploads.lock().unwrap();
            let offset = req.query("offset").and_then(|o| o.parse().ok());
            inbound::chunk(&store, &target, device, id, offset, &req.body)
        }
        ("POST", ["v1", "recordings", id, "complete"]) if valid_uuid(id) => {
            let (response, arrived) = {
                let _one = shared.uploads.lock().unwrap();
                inbound::complete(&store, &target, device, id)
            };
            if let Some(a) = arrived {
                (shared.events)(SyncEvent::Received {
                    document_id: a.document_id,
                    path: a.path,
                });
            }
            response
        }
        ("DELETE", ["v1", "devices", "self"]) => match store.remove_device(device) {
            Ok(()) => {
                (shared.events)(SyncEvent::DevicesChanged);
                Response::ok(json!({}))
            }
            Err(e) => {
                tracing::error!("unpairing a phone: {e}");
                Response::error(500, "internal", "Fennec could not remove this phone.")
            }
        },
        _ => Response::error(404, "not_found", "Fennec does not know this request."),
    }
}

/// `GET /v1/info`: this computer, and the projects and templates a
/// recording can be filed under.
fn info(shared: &Shared, store: &Store) -> Response {
    let projects: Vec<_> = store
        .projects()
        .unwrap_or_default()
        .into_iter()
        .map(|p| json!({ "id": p.id, "name": p.name, "color": p.color }))
        .collect();
    let templates: Vec<_> = crate::template::load_dir(&shared.templates)
        .into_iter()
        .flatten()
        .map(|t| json!({ "id": t.id, "name": t.name }))
        .collect();
    Response::ok(json!({
        "name": shared.name,
        "id": &shared.pin[..12],
        "version": env!("CARGO_PKG_VERSION"),
        "protocol": PROTOCOL_VERSION,
        "projects": projects,
        "templates": templates,
    }))
}

#[derive(Deserialize)]
struct PairBody {
    token: String,
    device_name: String,
    nonce: String,
}

/// `POST /v1/pair`: waits for Allow or Deny in Fennec.
fn pair(shared: &Shared, req: &Request) -> Response {
    let body: PairBody = match serde_json::from_slice(&req.body) {
        Ok(b) => b,
        Err(e) => return Response::error(400, "bad_request", &e.to_string()),
    };
    let nonce_ok = (16..=128).contains(&body.nonce.len())
        && body
            .nonce
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_');
    if !nonce_ok {
        return Response::error(
            400,
            "bad_nonce",
            "The nonce must be 16 to 128 letters, digits, - or _.",
        );
    }
    let token: String = body.token.chars().filter(|c| !c.is_whitespace()).collect();
    let name: String = body.device_name.trim().chars().take(64).collect();
    let name = if name.is_empty() {
        "Phone".to_string()
    } else {
        name
    };

    let (tx, rx) = crossbeam_channel::bounded(1);
    let presented = shared.pairing.lock().unwrap().present(&token, Instant::now(), tx);
    let request = match presented {
        Ok(r) => r,
        Err((refusal, closed)) => {
            if closed {
                (shared.events)(SyncEvent::PairingEnded);
            }
            return match refusal {
                Refusal::NoOffer => Response::error(
                    403,
                    "no_offer",
                    "Fennec is not pairing right now. Choose Pair a phone in Settings → Phone.",
                ),
                Refusal::Busy => {
                    Response::error(403, "busy", "Another phone is pairing. Try again in a moment.")
                }
                Refusal::WrongToken if closed => Response::error(
                    403,
                    "locked",
                    "Too many wrong codes. Start pairing again in Fennec.",
                ),
                Refusal::WrongToken => Response::error(403, "wrong_token", "That pairing code is wrong."),
            };
        }
    };
    (shared.events)(SyncEvent::PairRequest {
        request,
        device_name: name.clone(),
        code: pairing_code(&shared.pin, &token, &body.nonce),
    });
    let answer = rx.recv_timeout(shared.pair_wait);
    if answer.is_err() {
        shared.pairing.lock().unwrap().abandon(request);
    }
    let response = match answer {
        Ok(true) => {
            let secret = URL_SAFE_NO_PAD.encode(random_bytes::<32>());
            match Store::open(&shared.db).and_then(|s| s.add_device(&name, &secret_hash(&secret))) {
                Ok(device_id) => {
                    (shared.events)(SyncEvent::DevicesChanged);
                    Response::ok(json!({
                        "device_id": device_id,
                        "secret": secret,
                        "name": shared.name,
                        "id": &shared.pin[..12],
                    }))
                }
                Err(e) => {
                    tracing::error!("saving a paired phone: {e}");
                    Response::error(500, "internal", "Fennec could not save the pairing.")
                }
            }
        }
        Ok(false) => Response::error(403, "denied", "Pairing was denied in Fennec."),
        Err(_) => Response::error(403, "timeout", "Nobody pressed Allow in Fennec in time."),
    };
    (shared.events)(SyncEvent::PairingEnded);
    response
}
