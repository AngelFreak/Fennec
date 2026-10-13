//! Pairing a phone: a short-lived 8-digit token (in the QR code, or typed),
//! a code both screens show, and Allow pressed in Fennec.

use std::collections::HashMap;
use std::time::{Duration, Instant};

use crossbeam_channel::Sender;
use sha2::{Digest, Sha256};

/// How long a pairing offer stays open.
pub const OFFER_LIFETIME: Duration = Duration::from_secs(5 * 60);
/// Wrong tokens before the offer closes.
pub const MAX_FAILURES: u32 = 5;

/// What Settings → Phone shows while waiting for a phone.
#[derive(Debug, Clone, PartialEq)]
pub struct PairingOffer {
    /// `ip:port` the phone connects to.
    pub address: String,
    /// 8 digits.
    pub token: String,
    pub pin: String,
    /// This computer's name.
    pub name: String,
    pub expires_at: Instant,
}

impl PairingOffer {
    /// What the QR code holds.
    pub fn uri(&self) -> String {
        format!(
            "fennec://pair?h={}&pin={}&t={}&n={}",
            self.address,
            self.pin,
            self.token,
            encode(&self.name)
        )
    }

    /// The token as people read it: `7305 1148`.
    pub fn token_display(&self) -> String {
        split_digits(&self.token)
    }
}

/// The code both screens show, from the pin the phone saw, the token it
/// sent and its random nonce: `4821 9306`. A different code on the phone
/// means it is not talking to this Fennec.
pub fn pairing_code(pin: &str, token: &str, nonce: &str) -> String {
    let digest = Sha256::digest(format!("{pin}|{token}|{nonce}").as_bytes());
    let n = u64::from_be_bytes(digest[..8].try_into().expect("8 bytes")) % 100_000_000;
    split_digits(&format!("{n:08}"))
}

fn split_digits(d: &str) -> String {
    format!("{} {}", &d[..4], &d[4..])
}

fn encode(s: &str) -> String {
    s.bytes()
        .map(|b| match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' => (b as char).to_string(),
            _ => format!("%{b:02X}"),
        })
        .collect()
}

fn new_token() -> String {
    let n = u64::from_be_bytes(super::random_bytes::<8>()) % 100_000_000;
    format!("{n:08}")
}

struct Offer {
    token: String,
    expires_at: Instant,
    failures: u32,
    /// A phone with the right token is waiting for Allow.
    pending: bool,
}

/// Why a token was refused.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Refusal {
    /// No offer open (none made, expired, used or locked).
    NoOffer,
    /// Another phone is already waiting for Allow.
    Busy,
    WrongToken,
}

/// The receiver's pairing state, behind its mutex.
#[derive(Default)]
pub struct Pairing {
    offer: Option<Offer>,
    waiting: HashMap<u64, Sender<bool>>,
    next_request: u64,
}

impl Pairing {
    /// Opens a new offer, replacing any earlier one. Returns its token.
    pub fn open(&mut self, now: Instant) -> (String, Instant) {
        self.deny_waiting();
        let token = new_token();
        let expires_at = now + OFFER_LIFETIME;
        self.offer = Some(Offer {
            token: token.clone(),
            expires_at,
            failures: 0,
            pending: false,
        });
        (token, expires_at)
    }

    #[cfg(test)]
    fn is_open(&self, now: Instant) -> bool {
        self.offer.as_ref().is_some_and(|o| o.expires_at > now)
    }

    /// Closes the offer; a phone waiting for Allow is denied.
    pub fn close(&mut self) {
        self.offer = None;
        self.deny_waiting();
    }

    fn deny_waiting(&mut self) {
        for (_, tx) in self.waiting.drain() {
            let _ = tx.send(false);
        }
    }

    /// Checks a token. On success the offer waits for Allow under the
    /// returned request id; `answer` receives the decision. Returns whether
    /// a refusal also closed the offer.
    pub fn present(
        &mut self,
        token: &str,
        now: Instant,
        answer: Sender<bool>,
    ) -> Result<u64, (Refusal, bool)> {
        let Some(offer) = self.offer.as_mut().filter(|o| o.expires_at > now) else {
            self.offer = None;
            return Err((Refusal::NoOffer, false));
        };
        if offer.pending {
            return Err((Refusal::Busy, false));
        }
        if !constant_time_eq(offer.token.as_bytes(), token.as_bytes()) {
            offer.failures += 1;
            let locked = offer.failures >= MAX_FAILURES;
            if locked {
                self.offer = None;
            }
            return Err((Refusal::WrongToken, locked));
        }
        offer.pending = true;
        self.next_request += 1;
        let id = self.next_request;
        self.waiting.insert(id, answer);
        Ok(id)
    }

    /// Allow or Deny for a waiting phone. Either way the offer is used up.
    pub fn decide(&mut self, request: u64, allow: bool) {
        if let Some(tx) = self.waiting.remove(&request) {
            let _ = tx.send(allow);
            self.offer = None;
        }
    }

    /// The phone stopped waiting (timeout). Ends the offer.
    pub fn abandon(&mut self, request: u64) {
        if self.waiting.remove(&request).is_some() {
            self.offer = None;
        }
    }
}

fn constant_time_eq(a: &[u8], b: &[u8]) -> bool {
    a.len() == b.len() && a.iter().zip(b).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn codes_are_eight_digits_and_depend_on_every_input() {
        let c = pairing_code("pin", "12345678", "nonce");
        assert_eq!(c.len(), 9);
        assert!(c[..4].bytes().all(|b| b.is_ascii_digit()) && &c[4..5] == " ");
        assert_eq!(c, pairing_code("pin", "12345678", "nonce"));
        assert_ne!(c, pairing_code("pin2", "12345678", "nonce"));
        assert_ne!(c, pairing_code("pin", "12345679", "nonce"));
        assert_ne!(c, pairing_code("pin", "12345678", "nonce2"));
        // The phone app's tests check the same value (PairingCodeTest.kt).
        assert_eq!(
            pairing_code(
                "ODl9kE1pZ0yJ4o3b8zq5H2wTn7cVfR6sXuAaQeLgMtY",
                "73051148",
                "ui-test-nonce-0123456789"
            ),
            "8428 0745"
        );
    }

    #[test]
    fn the_qr_code_names_address_pin_token_and_computer() {
        let offer = PairingOffer {
            address: "192.168.1.20:47130".into(),
            token: "73051148".into(),
            pin: "abc_-D".into(),
            name: "Ane's laptop".into(),
            expires_at: Instant::now(),
        };
        assert_eq!(
            offer.uri(),
            "fennec://pair?h=192.168.1.20:47130&pin=abc_-D&t=73051148&n=Ane%27s%20laptop"
        );
        assert_eq!(offer.token_display(), "7305 1148");
    }

    #[test]
    fn an_offer_takes_one_phone_and_locks_after_wrong_tokens() {
        let now = Instant::now();
        let mut p = Pairing::default();
        let (tx, rx) = crossbeam_channel::bounded(1);
        assert_eq!(
            p.present("00000000", now, tx.clone()),
            Err((Refusal::NoOffer, false))
        );

        let (token, _) = p.open(now);
        assert_eq!(token.len(), 8);
        let id = p.present(&token, now, tx.clone()).unwrap();
        assert_eq!(p.present(&token, now, tx.clone()), Err((Refusal::Busy, false)));
        p.decide(id, true);
        assert_eq!(rx.try_recv(), Ok(true));
        assert!(!p.is_open(now), "used up");

        let (token, _) = p.open(now);
        let wrong = if token == "00000000" {
            "11111111"
        } else {
            "00000000"
        };
        for i in 1..MAX_FAILURES {
            assert_eq!(
                p.present(wrong, now, tx.clone()),
                Err((Refusal::WrongToken, false)),
                "try {i}"
            );
        }
        assert_eq!(
            p.present(wrong, now, tx.clone()),
            Err((Refusal::WrongToken, true))
        );
        assert_eq!(p.present(&token, now, tx), Err((Refusal::NoOffer, false)));
    }

    #[test]
    fn offers_expire_and_closing_denies_a_waiting_phone() {
        let now = Instant::now();
        let mut p = Pairing::default();
        let (tx, rx) = crossbeam_channel::bounded(1);
        let (token, expires) = p.open(now);
        assert_eq!(
            p.present(&token, expires, tx.clone()),
            Err((Refusal::NoOffer, false))
        );

        let (token, _) = p.open(now);
        p.present(&token, now, tx).unwrap();
        p.close();
        assert_eq!(rx.try_recv(), Ok(false));
    }
}
