//! One transfer: a ticket in the QR code, used once by the phone that scans
//! it first, which gets a session for the rest of the transfer.

use std::time::Instant;

use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;

/// Wrong tickets before the code stops working.
pub const MAX_FAILURES: u32 = 5;

/// What the Receive from phone dialog shows.
#[derive(Debug, Clone, PartialEq)]
pub struct SendOffer {
    /// `ip:port` the phone connects to.
    pub address: String,
    pub ticket: String,
    pub pin: String,
    /// This computer's name.
    pub name: String,
}

impl SendOffer {
    /// What the QR code holds.
    pub fn uri(&self) -> String {
        format!(
            "fennec://send?h={}&pin={}&t={}&n={}",
            self.address,
            self.pin,
            self.ticket,
            encode(&self.name)
        )
    }
}

fn encode(s: &str) -> String {
    s.bytes()
        .map(|b| match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' => (b as char).to_string(),
            _ => format!("%{b:02X}"),
        })
        .collect()
}

fn random_text<const N: usize>() -> String {
    URL_SAFE_NO_PAD.encode(super::random_bytes::<N>())
}

/// Why a ticket was refused.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Refusal {
    /// No code open: closed, or locked after wrong tickets.
    NoTicket,
    /// Another phone already scanned it.
    Used,
    WrongTicket,
}

enum State {
    /// The code is shown; nobody has scanned it.
    Open { ticket: String, failures: u32 },
    /// A phone is sending.
    Claimed { session: String },
    /// Done, stopped or locked.
    Closed,
}

/// The receiver's transfer, behind its mutex.
pub struct Session {
    state: State,
    /// Last time a phone did something, for the dialog's idle timeout.
    pub last_activity: Instant,
}

impl Session {
    /// A new, unscanned code.
    pub fn open(now: Instant) -> Self {
        Self {
            state: State::Open {
                ticket: random_text::<16>(),
                failures: 0,
            },
            last_activity: now,
        }
    }

    /// The ticket for the QR code, while nobody has scanned it.
    pub fn ticket(&self) -> Option<&str> {
        match &self.state {
            State::Open { ticket, .. } => Some(ticket),
            _ => None,
        }
    }

    /// Trades the ticket for a session. Returns the session, or the refusal
    /// and whether it also closed the code.
    pub fn claim(&mut self, ticket: &str, now: Instant) -> Result<String, (Refusal, bool)> {
        match &mut self.state {
            State::Closed => Err((Refusal::NoTicket, false)),
            State::Claimed { .. } => Err((Refusal::Used, false)),
            State::Open { ticket: t, failures } => {
                if !constant_time_eq(t.as_bytes(), ticket.as_bytes()) {
                    *failures += 1;
                    let locked = *failures >= MAX_FAILURES;
                    if locked {
                        self.state = State::Closed;
                    }
                    return Err((Refusal::WrongTicket, locked));
                }
                let session = random_text::<32>();
                self.state = State::Claimed {
                    session: session.clone(),
                };
                self.last_activity = now;
                Ok(session)
            }
        }
    }

    /// Whether `bearer` is the running session; counts as activity.
    pub fn admits(&mut self, bearer: Option<&str>, now: Instant) -> bool {
        let ok = match (&self.state, bearer) {
            (State::Claimed { session }, Some(b)) => constant_time_eq(session.as_bytes(), b.as_bytes()),
            _ => false,
        };
        if ok {
            self.last_activity = now;
        }
        ok
    }

    pub fn close(&mut self) {
        self.state = State::Closed;
    }
}

fn constant_time_eq(a: &[u8], b: &[u8]) -> bool {
    a.len() == b.len() && a.iter().zip(b).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_qr_code_names_address_pin_ticket_and_computer() {
        let offer = SendOffer {
            address: "192.168.1.20:47130".into(),
            ticket: "q9Xh2kLmN0pQrStUvWxYz1".into(),
            pin: "abc_-D".into(),
            name: "Ane's laptop".into(),
        };
        assert_eq!(
            offer.uri(),
            "fennec://send?h=192.168.1.20:47130&pin=abc_-D&t=q9Xh2kLmN0pQrStUvWxYz1&n=Ane%27s%20laptop"
        );
    }

    #[test]
    fn a_ticket_is_used_once_and_its_session_admits_only_itself() {
        let now = Instant::now();
        let mut s = Session::open(now);
        let ticket = s.ticket().unwrap().to_string();
        assert_eq!(ticket.len(), 22);
        assert!(!s.admits(None, now));

        let session = s.claim(&ticket, now).unwrap();
        assert_eq!(session.len(), 43);
        assert_eq!(s.ticket(), None, "no longer shown");
        assert_eq!(s.claim(&ticket, now), Err((Refusal::Used, false)));
        assert!(s.admits(Some(&session), now));
        assert!(!s.admits(Some(&ticket), now));
        assert!(!s.admits(Some("x"), now));

        s.close();
        assert!(!s.admits(Some(&session), now));
        assert_eq!(s.claim(&ticket, now), Err((Refusal::NoTicket, false)));
    }

    #[test]
    fn wrong_tickets_close_the_code() {
        let now = Instant::now();
        let mut s = Session::open(now);
        let ticket = s.ticket().unwrap().to_string();
        for i in 1..MAX_FAILURES {
            assert_eq!(
                s.claim("wrong", now),
                Err((Refusal::WrongTicket, false)),
                "try {i}"
            );
        }
        assert_eq!(s.claim("wrong", now), Err((Refusal::WrongTicket, true)));
        assert_eq!(s.claim(&ticket, now), Err((Refusal::NoTicket, false)));
    }
}
