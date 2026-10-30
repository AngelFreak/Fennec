//! Receive from phone: the code to scan, then a phone sending two
//! recordings, one arrived and one halfway (a stand-in phone on a thread
//! speaks the protocol).

use std::time::Duration;

use fennec::ui::Nav;

use crate::{Scene, pump_until};

pub fn capture(s: &Scene) {
    s.w.sidebar.go(Nav::Files);
    s.w.phone.receive();
    let Some(offer) = s.w.phone.offer() else {
        println!("FAILED no code to scan: {:?}", s.w.phone.transfer());
        return;
    };
    s.shot("receive-phone");

    let base = format!("https://{}", offer.address);
    let phone = std::thread::spawn(move || {
        let client = reqwest::blocking::Client::builder()
            .danger_accept_invalid_certs(true)
            .build()
            .unwrap();
        let hello: serde_json::Value = client
            .post(format!("{base}/v1/hello"))
            .json(&serde_json::json!({ "ticket": offer.ticket, "device_name": "Pixel 8" }))
            .send()
            .and_then(|r| r.json())
            .unwrap_or_default();
        let Some(session) = hello["session"].as_str() else {
            return;
        };
        let send = |id: &str, title: &str, audio: &[u8], sent: usize, ext: &str| {
            use sha2::Digest;
            let sum: String = sha2::Sha256::digest(audio)
                .iter()
                .map(|b| format!("{b:02x}"))
                .collect();
            let meta = serde_json::json!({
                "title": title, "recorded_at": 1_791_000_000_000_i64, "duration_ms": 1_800_000,
                "ext": ext, "size": audio.len(), "sha256": sum,
            });
            let _ = client
                .put(format!("{base}/v1/recordings/{id}"))
                .bearer_auth(session)
                .json(&meta)
                .send();
            let mut offset = 0;
            while offset < sent {
                let n = (sent - offset).min(1 << 20);
                let _ = client
                    .put(format!("{base}/v1/recordings/{id}/audio?offset={offset}"))
                    .bearer_auth(session)
                    .body(audio[offset..offset + n].to_vec())
                    .send();
                offset += n;
            }
            if sent == audio.len() {
                let _ = client
                    .post(format!("{base}/v1/recordings/{id}/complete"))
                    .bearer_auth(session)
                    .send();
            }
        };
        let wav = std::fs::read(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/tests/fixtures/da_fleurs_0.wav"
        ))
        .unwrap();
        send(
            "aaaaaaaa-1111-2222-3333-444444444444",
            "Byggemøde, Havnegade",
            &wav,
            wav.len(),
            "wav",
        );
        let long = vec![0u8; 6_400_000];
        send(
            "bbbbbbbb-1111-2222-3333-444444444444",
            "Interview med afdelingslederen",
            &long,
            2_600_000,
            "aac",
        );
    });
    let _ = phone.join();
    pump_until(Duration::from_secs(5), || {
        s.w.phone.dialog_page().as_deref() == Some("sending")
    });
    s.shot("receive-sending");
    s.w.phone.close();
}
