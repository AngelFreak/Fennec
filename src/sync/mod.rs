//! Receiving recordings from Fennec Recorder, the Android companion app.
//!
//! While receiving is on, Fennec runs a small HTTPS server on the local
//! network. A phone pairs once by scanning a QR code and having someone press
//! Allow here; after that it uploads recordings in chunks, and each finished
//! recording becomes a document in the Files queue. Nothing leaves the
//! network. The protocol is described in
//! `docs/plans/2026-10-06-android-recorder.md`.

mod discovery;
mod http;
mod inbound;
mod pairing;
mod projects;
mod server;
mod tls;
pub mod usb;

use std::path::PathBuf;
use std::sync::Arc;

use sha2::{Digest, Sha256};

use crate::store::DocumentId;

pub use inbound::{Arrived, Target};
pub use pairing::{PairingOffer, pairing_code};
pub use server::{Receiver, ReceiverConfig};
pub use tls::Identity;

/// Port Fennec listens on unless Settings says otherwise.
pub const DEFAULT_PORT: u16 = 47130;
/// Version of the phone protocol, reported by `GET /v1/info`.
pub const PROTOCOL_VERSION: u32 = 1;

/// What the receiver tells the app. Sent from the receiver's threads.
#[derive(Debug, Clone, PartialEq)]
pub enum SyncEvent {
    /// A phone presented the pairing code. Show `code` and answer with
    /// [`Receiver::decide`].
    PairRequest {
        request: u64,
        device_name: String,
        code: String,
    },
    /// The pairing offer is over: used, denied, cancelled, expired or
    /// locked after wrong codes.
    PairingEnded,
    /// A phone was paired or removed.
    DevicesChanged,
    /// A phone added a project or changed one.
    ProjectsChanged,
    /// A recording arrived in full and has a document; it should join the
    /// Files queue.
    Received { document_id: DocumentId, path: PathBuf },
}

pub type EventSink = Arc<dyn Fn(SyncEvent) + Send + Sync>;

/// The computer's name as phones show it.
pub fn desktop_name() -> String {
    std::fs::read_to_string("/proc/sys/kernel/hostname")
        .ok()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "Fennec".into())
}

fn sha256_hex(bytes: &[u8]) -> String {
    hex(&Sha256::digest(bytes))
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

fn random_bytes<const N: usize>() -> [u8; N] {
    let mut buf = [0u8; N];
    getrandom::fill(&mut buf).expect("the system random source works");
    buf
}

/// How a device secret is kept in the database: only its hash.
fn secret_hash(secret: &str) -> String {
    sha256_hex(secret.as_bytes())
}
