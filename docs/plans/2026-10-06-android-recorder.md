# Fennec Recorder — Android companion

A phone app that records meetings and interviews and hands the audio to
Fennec on your computer, which transcribes it with the models it already has.
Status: proposed and built 2026-10-06 (all five stages); tested end to end
with the app in the Android emulator. UI mockup:
https://claude.ai/artifact/F1XMmhXxkgaciWFxH7h7sx

## Goals

- **Record on the phone**, reliably: screen off, other apps open, an hour or
  more. Recordings are kept on the phone until Fennec confirms it has them.
- **Send to Fennec over the home network** once paired. No server, account
  or cloud relay; audio goes from the phone to your own computer and nowhere
  else.
- **Arrive as ordinary documents**: a phone recording lands in Fennec's Files
  queue, is transcribed like a dropped-in file, and keeps its audio for the
  player.
- **Show progress on the phone**: waiting, sending, queued, transcribing,
  done.
- **Look like Fennec**: same colours, fonts and shapes as the desktop app.

Non-goals for v1: transcribing on the phone, dictating into other Android
apps (a keyboard), reading or editing transcripts on the phone, sync over
the internet, iOS.

## UI

English UI; titles and content are Danish. Screens (see mockup):

1. **Record**: project and template chips, a large timer, a level meter, one
   record/stop button. Recording runs as a foreground service with a
   notification that has Pause and Stop.
2. **Recordings**: grouped by day; each row shows title, length and a sync
   chip (Waiting for Fennec / Sending n% / Queued / Transcribing / Done /
   Failed). A footer line shows the paired computer and whether it is
   reachable.
3. **Recording detail**: rename, change project and template before it is
   sent, play back, see the sync steps, delete from the phone.
4. **Pair**: scan the QR code from Fennec, compare the code, confirm.
5. **Settings**: paired computer (unpair), audio quality, send only on
   unmetered Wi-Fi, keep recordings on the phone for N days after sending.

Desktop: a new **Settings → Phone** section. A switch to receive recordings
and the port, "Pair a phone" (the QR code, plus the address and code to type
for phones without a camera, and a countdown), the list of paired phones
(name, last seen, recordings sent, Remove) and the project phone recordings go
to when none was chosen. When a phone presents the code, an Allow / Deny
dialog with the code to compare opens wherever Fennec is. Recordings are
always transcribed on arrival; the mockup's "Transcribe as soon as it
arrives" switch was left out.

### Design system on Android

The Android theme is Fennec's tokens, not Material's defaults:

- **Colours**: Material 3 `lightColorScheme` / `darkColorScheme` filled from
  `src/ui/style-light.css` and `style-dark.css`: `primary` = `fx_accent`
  (`#C2410C` / `#E0652C`), `surface` = `fx_surface`, `surfaceVariant` =
  `fx_chrome`, `outline` = `fx_border`, `outlineVariant` = `fx_divider`,
  `onSurfaceVariant` = `fx_muted`, `error` = `fx_error`. Status chips use
  `fx_accent_soft`/`fx_accent_text` (sending), `fx_net_bg`/`fx_net_text`
  (done) and `fx_chip`/`fx_muted` (waiting). Dynamic colour is off.
- **Type**: IBM Plex Sans for the interface, IBM Plex Mono for timers,
  lengths and codes, Source Serif 4 for recording titles. All three are OFL
  and bundled (Source Serif 4 is already in `data/fonts/`).
- **Shapes**: 8 px controls, 10 px boxes, pill chips; 1 px borders, no
  shadows; spaced capitals for section titles ("TODAY"), as in the desktop
  sidebar.
- **Drift**: in v1 the values are copied by hand and a desktop test parses
  both CSS files and a generated `android/…/FennecColors.kt` to check they
  match. Later, one `design/tokens.toml` can generate both.

## Protocol (v1)

### Discovery

Fennec advertises `_fennec._tcp` over mDNS while receiving is on (TXT:
`id=<desktop id>`, `v=1`, `name=<hostname>`). The phone finds it with
`NsdManager`, and also remembers the last address so a network without mDNS
still works. Port: fixed default 47130, configurable.

### Transport

HTTPS with a self-signed certificate Fennec generates the first time
receiving is turned on (`rcgen`, stored in `~/.local/share/fennec/sync/`).
The phone pins its SPKI SHA-256 from the QR code (OkHttp
`CertificatePinner`), so another machine on the network cannot pose as
Fennec. Every request after pairing carries `Authorization: Bearer
<device secret>`; Fennec stores only a hash of the secret.

### Pairing

1. Settings → Phone → Pair a phone (this also turns receiving on). Fennec
   makes a one-time 8-digit token, valid 5 minutes, and shows a QR code:
   `fennec://pair?h=<ip:port>&pin=<pin>&t=<token>&n=<computer name>`.
   `pin` is SHA-256 of the certificate's SubjectPublicKeyInfo in unpadded
   URL-safe base64; `n` is percent-encoded. The same address and token are
   shown as text for phones without a camera (`7305 1148`; spaces in the
   token are ignored).
2. The phone scans it (zxing, on-device; or another camera app opens the
   `fennec://pair` link, and the app asks before connecting),
   connects with the pin and sends `POST /v1/pair {token, device_name,
   nonce}` (nonce: 16–128 random letters, digits, `-` or `_`).
3. Both screens show the same 8-digit code: the first 8 bytes of
   SHA-256(`"<pin>|<token>|<nonce>"`) as a big-endian integer, modulo
   10⁸, zero-padded, shown as `4821 9306`. Fennec asks Allow / Deny; the
   phone's request stays open until then (up to 120 s, so the phone's read
   timeout must be longer) and returns `{device_id, secret, name, id}`.
   The secret is kept in Android Keystore-backed storage.
4. A phone that typed the address trusts the certificate it is shown on
   first use; the code comparison in step 3 is what protects it.
5. Five wrong tokens close the offer. Only one phone can wait for Allow at a
   time. An offer is used up by Allow, Deny or a timeout.

### Endpoints

Every answer is JSON; errors are `{"error": <code>, "message": <text to
show>}`. Every request except pairing carries `Authorization: Bearer
<secret>`, or gets 401 `not_paired`. One request per connection
(`Connection: close`); bodies need a Content-Length (no chunked encoding)
and may be up to 4 MB.

| Method and path | Purpose |
|---|---|
| `POST /v1/pair` | Pair (above). 403 with `no_offer`, `busy`, `wrong_token`, `locked`, `denied` or `timeout` |
| `GET /v1/info` | `{name, id, version, protocol: 1, projects: [{id, name, color, default_template, documents}], templates: [{id, name, fields: [{key, label, kind, required}]}], default_template, project_colors}` |
| `POST /v1/projects` | Add a project: `{name, color?, default_template?}`. 409 `exists` for a name Fennec already has (any case), 400 `bad_name`, `bad_color` (`#RRGGBB`), `bad_template` |
| `PUT /v1/projects/{id}` | Change `name`, `color` and/or `default_template` (`null` clears it); fields left out stay. Deleting stays on the computer |
| `PUT /v1/recordings/{id}` | Announce: `{title, recorded_at (ms), duration_ms, project_id?, template_id?, ext? (default m4a), size, sha256 (hex)}`. Repeating it is harmless; answers the recording's status (below), so `received` says where to resume. A different size or checksum under the same id while still arriving starts over |
| `PUT /v1/recordings/{id}/audio?offset=N` | One chunk (the phone sends 1 MB). If `N` is not where the stored part ends: 409 `offset` with `received` |
| `POST /v1/recordings/{id}/complete` | Fennec checks size and SHA-256, makes the document and queues it. 409 `incomplete` (with `received`), 422 `checksum` (the part is deleted; `received` is 0, send again). Repeating it returns the same document |
| `GET /v1/recordings?ids=a,b` | `{recordings: [status…]}`; ids Fennec does not know (or another phone's) are `{"id", "state": "unknown"}` |
| `DELETE /v1/devices/self` | Unpair from the phone |

A status is `{id, received, state, document_id, error}` with `state` one of
`receiving`, `queued`, `transcribing`, `done`, `failed` (`error` says why).
Recording ids are made on the phone when recording starts (8–64 letters,
digits and `-`; a UUID), so a retried upload or a repeated `complete` never
makes a second document. Extensions accepted: m4a, aac, mp3, ogg, opus, wav,
flac, webm. Largest recording: 4 GB. Uploads that stop arriving are deleted
after 7 days.

### Audio

Raw AAC (ADTS, `.aac`), mono, 16 kHz, 48 kbit/s (about 21 MB an hour).
Unlike `.m4a`, ADTS needs no index written when recording stops, so a
recording cut short (the app killed, the battery dead) is still playable up
to where it stopped; `src/audio/decode.rs` tests that with a real recording
from the app, whole and cut in half. 16 kHz mono is what the engine
resamples to anyway. A "high quality" setting records 48 kHz at 128 kbit/s
for people who also want the audio itself.

## Desktop changes

```
src/
  sync/                  no GTK
    mod.rs               events, helpers
    server.rs            Receiver: listener thread, routes (one thread per connection)
    http.rs              the little HTTP/1.1 it needs
    tls.rs               the certificate and its pin
    pairing.rs           tokens, codes, waiting for Allow
    inbound.rs           announce, chunks, checksum, document
    discovery.rs         mDNS advertisement (mdns-sd)
  store/phone.rs         devices and inbound_recordings
  ui/phone.rs            PhoneLink: runs the receiver, Allow dialog, hand-off to Files
  ui/settings_phone.rs   Settings → Phone
```

- **Blocking, like the rest.** No async runtime (see the AI section of the
  design). `rustls` over `std::net::TcpListener`, a small router, a thread
  per connection, its own store connection per request. Crates:
  `rustls` (ring), `rcgen`, `httparse`, `mdns-sd`, `qrcode` (drawn with
  cairo, always black on white so cameras read it in dark mode too).
- **Store** (migration 5): new tables
  `devices(id, name, secret_hash, paired_at, last_seen_at)` and
  `inbound_recordings(uuid PRIMARY KEY, device_id, title, recorded_at,
  duration_ms, project_id NULL, template_id NULL, ext, size, sha256,
  received, state, document_id NULL, error NULL, updated_at)`. A recording
  without a template of its own gets its project's default template, else
  Fennec's default, as in dictation. Phone documents keep
  `source = 'file'`: a new value would mean rebuilding the `documents`
  table to change its CHECK constraint, and the inbound row already records
  where a document came from. The document is dated by
  `recorded_at`, not the arrival time.
- **Migrations** now run inside one write-locked transaction, and opening
  a store retries when SQLite answers "busy" at once: the receiver, the
  window and the audio clean-up can open a new database together.
- **Hand-off**: on `complete`, the file moves from `sync/incoming/` to the
  audio dir as `phone-<id>.<ext>`, the document is created with its audio
  path, and an event asks the Files screen to queue it. The Files queue
  writes each item's state back to `inbound_recordings`, which is what the
  phone reads. Recordings left queued or half transcribed when Fennec
  closed are transcribed again from the start when it opens, whether or not
  receiving is on.
- **Priority**: phone files run at file-chunk priority, below live
  dictation, exactly like dropped-in files.
- **Settings** (`settings.toml`, `[phone]`): `enabled` (off by default),
  `port` (47130), `default_project`.

## Android app

As built (`android/`, package `io.github.fennec.recorder`):

- Kotlin, Jetpack Compose, Material 3 themed as above. `minSdk` 29.
- `RecordingService`: foreground service, type `microphone`,
  `MediaRecorder` to raw AAC in the app's files; pause and resume (paused
  time is not in the recording); a notification with Pause and Stop. The
  row is written when recording starts, so after a crash the next start
  keeps whatever was recorded.
- Room for recordings and their sync state; the pairing in
  SharedPreferences with the device secret sealed by an Android Keystore
  key. No cloud backup or device transfer of either.
- `UploadWorker` (WorkManager): sends when the network allows (unmetered by
  default), resumes from Fennec's received offset, resends a damaged file
  once, stops on 401 (forgets the pairing), on a pin mismatch, or retries
  with backoff. When Fennec is not reachable it looks for it by mDNS and
  updates the address.
- `FollowWorker` and the open app poll `/v1/recordings` until each
  recording is done or failed; no push. `CleanupWorker` deletes transcribed
  recordings after the chosen days.
- A quick settings tile and a home screen widget open the app and start
  recording (Android requires the app in front to start the microphone).
- The Marker button from the mockup was dropped (see Open questions).
- Released as an APK on GitHub releases next to the `.deb`, signed with the
  key from the repository secrets.

## USB cable

For when the phone and computer share no Wi-Fi, without developer mode or
adb: Android's **File transfer** mode (MTP), which GNOME mounts through GVFS.

- **Phone**: each recording not yet sent also gets a copy in
  `Download/Fennec Recorder/` through MediaStore (no permission needed):
  `<id>.aac`, then `<id>.json` (`fennec_recorder: 1`, id, title,
  recorded_at, duration_ms, project_id, template_id, ext, size, sha256,
  device_id). `tests/fixtures/usb/sidecar.json` is checked by both apps.
  Edits rewrite the sidecar; delivery over Wi-Fi, deleting the recording, or
  turning copies off removes them.
- **Fennec** (`src/sync/usb.rs`, `src/ui/usb.rs`): watches GIO mounts; on an
  `mtp://` mount with that folder it removes copies it already has and asks
  "Import recordings from <phone>?". Importing copies the audio, checks the
  SHA-256, files it like an upload (`inbound::file_recording`, so project
  and template defaults match), then deletes both files from the phone.
  Settings → Phone → Import over USB turns it off.
- **Back to the phone**: Android shows an app only its own files, so Fennec
  cannot leave a receipt. It deletes the copy instead; the app checks its
  MediaStore entries (on start, every few seconds while open, around
  uploads) and marks a recording whose copy is gone **Sent by USB**. Once
  the phone reaches Fennec over Wi-Fi, `/v1/recordings` reports its real
  state (imports keep the phone's id; ones from an unpaired phone have no
  device and any paired phone may ask by id). **Mark as sent to Fennec** /
  **Mark as not sent** cover what the check cannot tell.
- **Tested**: desktop import against a phone-shaped folder and the real
  window (offer, import, transcript, removal, Not now); the app's copy logic
  with a fake MediaStore; on the emulator, a recording's copy pulled off
  the device and imported by the desktop code, then deleted on the device
  and marked Sent by USB. Not tested: a real MTP mount (the emulator cannot
  be one).

## Privacy and security

- Receiving is off by default and only listens while the switch is on.
- Only paired devices can upload; pairing needs someone at the desktop to
  press Allow. Removing a device revokes its secret at once.
- Audio never leaves the phone and computer pair. The phone has no network
  permission beyond the local network calls (no analytics, no crash
  reporting).
- Uploads are capped (4 GB a recording) and incoming files that never
  complete are deleted after 7 days.

## Error handling

Phone: Fennec not reachable → "Waiting for Fennec" with the last time it was
seen; pin mismatch → stop and ask to pair again, never fall back; 401 →
"This phone was removed in Fennec", offer to pair again; checksum mismatch →
resend from zero once, then Failed with the reason. Desktop: decode or
transcription failure shows on the Files item as today and is reported to
the phone as `failed` with the message.

## Testing

As built:

- **Desktop** (`tests/phone_sync.rs`): the real server on localhost and a
  Rust client pair, upload in chunks, resume after Fennec restarts, fail a
  damaged upload, refuse bad requests, unpair, and find Fennec over mDNS.
  `tests/ui.rs` pairs through the window, transcribes a WAV and resumes an
  interrupted transcription after a restart.
- **Android unit tests** (`./gradlew :app:testDebugUnitTest`): pairing links
  and the pairing code (one fixed value is checked on both sides),
  certificate pinning, uploads against a fake Fennec over HTTPS (a lost
  answer, a damaged file, a removed phone, a missing file), the desktop's QR
  code (a screenshot of it) read by the app's scanner, crash recovery,
  clean-up, the colour tokens against the desktop CSS, and screenshots of
  every screen in light and dark.
- **End to end** (`android/e2e/run-e2e.sh`): the real Fennec window,
  headless, with a stand-in speech engine (`tests/phone_e2e.rs`), and the
  app in the emulator through its UI: pair through Fennec's link, record,
  record with a pause, see both transcribed, unpair. The script then checks
  on the desktop that both arrived, decode to their recorded length, keep
  their title, date and project, and that the phone is unpaired. The
  release build and the quick settings tile were checked the same way by
  hand.
- **Not covered**: real speech (the emulator's microphone gives silence),
  mDNS from the phone (the emulator's network carries none), the camera
  itself, and the home screen widget.

## Stages

1. **Desktop receiver** (done): server, pairing, upload, Settings → Phone.
   Tested end to end by `tests/phone_sync.rs` (a stand-in phone over real
   HTTPS) and `tests/ui.rs` (pairing through the window, a WAV transcribed
   and reported `done` to the phone, resuming after a restart).
2. **Recorder, offline** (done): record, pause, list, play, rename, delete.
3. **Pairing and sending** (done): QR, link, typed code, upload worker,
   resume.
4. **Status and projects** (done): status chips, project and template
   pickers from `/v1/info`.
5. **Polish** (done): quick-settings tile to start recording, home-screen widget,
   release workflow for the APK.

## Open questions

- **Fennec closed**: v1 receives only while Fennec runs. A small background
  service that receives while the window is closed would help; decide after
  stage 3.
- **Away from home**: recordings wait. With Tailscale or similar the phone
  can reach Fennec anywhere with no protocol change; worth documenting, not
  building.
- **Markers**: the mockup's Record screen has a Marker button for "this
  part matters". Sending them is one more metadata field (`markers_ms`), but
  Fennec has nowhere to show them yet; perhaps highlight the paragraph at
  that time. Dropped from v1; the button is not in the app.
- **Transcript back to the phone**: read-only text after "done" is a small
  addition to `/v1/recordings`; left out of v1.
