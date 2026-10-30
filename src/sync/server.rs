//! The receiver: a TLS listener on its own thread, a thread per connection,
//! and the protocol's routes.

use std::net::{IpAddr, SocketAddr, TcpListener, TcpStream, UdpSocket};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use serde::Deserialize;
use serde_json::json;

use super::http::{ReadError, Request, Response, read_request, write_response};
use super::inbound::{self, Target, valid_uuid};
use super::projects;
use super::session::{Refusal, SendOffer, Session};
use super::tls::Identity;
use super::{EventSink, PROTOCOL_VERSION, SyncEvent, desktop_name};
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
}

impl ReceiverConfig {
    /// Every network interface on the port from Settings.
    pub fn from_settings(paths: &Paths, settings: &Settings) -> Self {
        Self {
            bind: SocketAddr::from(([0, 0, 0, 0], settings.phone.port)),
            paths: paths.clone(),
            default_project: settings.phone.default_project,
            default_template: settings.default_template.clone(),
        }
    }
}

struct Shared {
    db: PathBuf,
    templates: PathBuf,
    target: Mutex<Target>,
    /// Uploads touch files and rows together; one at a time.
    uploads: Mutex<()>,
    session: Mutex<Session>,
    events: EventSink,
    pin: String,
    name: String,
}

/// A running receiver for one transfer, with a fresh code. Stops when
/// dropped.
pub struct Receiver {
    shared: Arc<Shared>,
    stop: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
    local_addr: SocketAddr,
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
            session: Mutex::new(Session::open(Instant::now())),
            events,
            pin: identity.pin.clone(),
            name,
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
        })
    }

    pub fn local_addr(&self) -> SocketAddr {
        self.local_addr
    }

    /// The certificate pin phones check.
    pub fn pin(&self) -> &str {
        &self.shared.pin
    }

    /// What the QR code shows, until a phone scans it.
    pub fn offer(&self) -> Option<SendOffer> {
        let ticket = self.shared.session.lock().unwrap().ticket()?.to_string();
        let ip = match self.local_addr.ip() {
            ip if ip.is_unspecified() => lan_ip().unwrap_or(IpAddr::from([127, 0, 0, 1])),
            ip => ip,
        };
        Some(SendOffer {
            address: SocketAddr::new(ip, self.local_addr.port()).to_string(),
            ticket,
            pin: self.shared.pin.clone(),
            name: self.shared.name.clone(),
        })
    }

    /// When a phone last did something (or the code was shown).
    pub fn last_activity(&self) -> Instant {
        self.shared.session.lock().unwrap().last_activity
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
        self.shared.session.lock().unwrap().close();
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
    match (method, segs.as_slice()) {
        ("POST", ["v1", "hello"]) => return hello(shared, req),
        ("POST", ["v1", "pair"]) => {
            return Response::error(
                410,
                "update_app",
                "Update Fennec Recorder to send to this version of Fennec.",
            );
        }
        _ => {}
    }
    if !shared
        .session
        .lock()
        .unwrap()
        .admits(req.bearer(), Instant::now())
    {
        return Response::error(
            401,
            "no_session",
            "Fennec stopped receiving. Scan a new code in Fennec to send.",
        );
    }

    let store = match Store::open(&shared.db) {
        Ok(s) => s,
        Err(e) => {
            tracing::error!("phone request: {e}");
            return Response::error(503, "unavailable", "Fennec cannot open its database.");
        }
    };
    let target = shared.target.lock().unwrap().clone();
    match (method, segs.as_slice()) {
        ("PUT", ["v1", "recordings", id]) if valid_uuid(id) => {
            let response = {
                let _one = shared.uploads.lock().unwrap();
                inbound::announce(&store, &target, id, &req.body)
            };
            arriving(shared, &store, id, &response);
            response
        }
        ("PUT", ["v1", "recordings", id, "audio"]) if valid_uuid(id) => {
            let response = {
                let _one = shared.uploads.lock().unwrap();
                let offset = req.query("offset").and_then(|o| o.parse().ok());
                inbound::chunk(&store, &target, id, offset, &req.body)
            };
            arriving(shared, &store, id, &response);
            response
        }
        ("POST", ["v1", "recordings", id, "complete"]) if valid_uuid(id) => {
            let (response, arrived) = {
                let _one = shared.uploads.lock().unwrap();
                inbound::complete(&store, &target, id)
            };
            if let Some(a) = arrived {
                (shared.events)(SyncEvent::Received {
                    id: id.to_string(),
                    document_id: a.document_id,
                    path: a.path,
                });
            }
            response
        }
        ("POST", ["v1", "done"]) => {
            shared.session.lock().unwrap().close();
            (shared.events)(SyncEvent::Finished);
            Response::ok(json!({}))
        }
        _ => Response::error(404, "not_found", "Fennec does not know this request."),
    }
}

/// Tells the dialog how far a recording has come, after an announce or a
/// chunk it accepted.
fn arriving(shared: &Shared, store: &Store, id: &str, response: &Response) {
    if response.status != 200 {
        return;
    }
    if let Ok(Some(r)) = store.inbound(id) {
        (shared.events)(SyncEvent::Arriving {
            id: id.to_string(),
            title: r.info.title,
            received: response.body["received"].as_u64().unwrap_or(0),
            size: r.info.size,
        });
    }
}

#[derive(Deserialize)]
struct HelloBody {
    ticket: String,
    device_name: String,
}

/// `POST /v1/hello`: the phone that scanned the code trades its ticket for
/// a session, and learns the projects and templates it can file under.
fn hello(shared: &Shared, req: &Request) -> Response {
    let body: HelloBody = match serde_json::from_slice(&req.body) {
        Ok(b) => b,
        Err(e) => return Response::error(400, "bad_request", &e.to_string()),
    };
    let name: String = body.device_name.trim().chars().take(64).collect();
    let name = if name.is_empty() {
        "Phone".to_string()
    } else {
        name
    };
    let claimed = shared
        .session
        .lock()
        .unwrap()
        .claim(body.ticket.trim(), Instant::now());
    let session = match claimed {
        Ok(s) => s,
        Err((refusal, closed)) => {
            if closed {
                (shared.events)(SyncEvent::CodeClosed);
            }
            return match refusal {
                Refusal::NoTicket => Response::error(
                    403,
                    "no_ticket",
                    "This code has expired. Show a new one in Fennec.",
                ),
                Refusal::Used => Response::error(
                    403,
                    "used",
                    "This code was already used. Show a new one in Fennec.",
                ),
                Refusal::WrongTicket => Response::error(403, "wrong_ticket", "That code is wrong."),
            };
        }
    };
    let store = match Store::open(&shared.db) {
        Ok(s) => s,
        Err(e) => {
            tracing::error!("phone hello: {e}");
            shared.session.lock().unwrap().close();
            return Response::error(503, "unavailable", "Fennec cannot open its database.");
        }
    };
    (shared.events)(SyncEvent::Connected { device_name: name });
    let projects: Vec<_> = store
        .projects()
        .unwrap_or_default()
        .iter()
        .map(projects::project_json)
        .collect();
    let default_template = shared.target.lock().unwrap().default_template.clone();
    Response::ok(json!({
        "session": session,
        "name": shared.name,
        "id": &shared.pin[..12],
        "version": env!("CARGO_PKG_VERSION"),
        "protocol": PROTOCOL_VERSION,
        "projects": projects,
        "templates": projects::templates_json(&shared.templates),
        "default_template": default_template,
        "project_colors": crate::store::PROJECT_COLORS,
    }))
}
