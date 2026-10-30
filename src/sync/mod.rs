//! Receiving recordings from Fennec Recorder, the Android companion app.
//!
//! While the Receive from phone dialog is open, Fennec runs a small HTTPS
//! server on the local network and shows a QR code. The phone that scans it
//! uploads its recordings in chunks, and each finished recording becomes a
//! document in the Files queue. Nothing leaves the network, and nothing is
//! kept for the next time. The protocol is described in
//! `docs/plans/2026-10-30-send-by-qr.md`.

mod http;
mod inbound;
mod projects;
mod server;
mod session;
mod tls;
pub mod usb;

use std::path::PathBuf;
use std::sync::Arc;

use crate::store::DocumentId;

pub use inbound::{Arrived, Target};
pub use server::{Receiver, ReceiverConfig};
pub use session::SendOffer;
pub use tls::Identity;

/// Port Fennec listens on unless Settings says otherwise.
pub const DEFAULT_PORT: u16 = 47130;
/// Version of the phone protocol, reported by `POST /v1/hello`.
pub const PROTOCOL_VERSION: u32 = 2;

/// What the receiver tells the app. Sent from the receiver's threads.
#[derive(Debug, Clone, PartialEq)]
pub enum SyncEvent {
    /// A phone scanned the code and is sending.
    Connected { device_name: String },
    /// A recording is arriving: `received` of `size` bytes are here.
    Arriving {
        id: String,
        title: String,
        received: u64,
        size: u64,
    },
    /// A recording arrived in full and has a document; it should join the
    /// Files queue.
    Received {
        id: String,
        document_id: DocumentId,
        path: PathBuf,
    },
    /// The phone has sent everything.
    Finished,
    /// Too many wrong codes: the code no longer works.
    CodeClosed,
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

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

fn random_bytes<const N: usize>() -> [u8; N] {
    let mut buf = [0u8; N];
    getrandom::fill(&mut buf).expect("the system random source works");
    buf
}
