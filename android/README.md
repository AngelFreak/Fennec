# Fennec Recorder

The Android companion to Fennec: record meetings and interviews on your phone,
and the audio goes to Fennec on your computer over your own network, where it
is transcribed with the models you already have. Nothing passes through the
internet. Android 10 or newer.

The design is in
[`docs/plans/2026-10-06-android-recorder.md`](../docs/plans/2026-10-06-android-recorder.md);
the phone protocol (v2, sending by QR code) in
[`docs/plans/2026-10-30-send-by-qr.md`](../docs/plans/2026-10-30-send-by-qr.md).

## Using it

1. Record. Recordings stay on the phone, marked **Not sent**.
2. In Fennec: **Receive from phone** (on the Files page, or Settings →
   Phone). A QR code appears; Fennec listens only while it is shown.
3. In Fennec Recorder: **Send to Fennec** and point the camera at the code
   (or scan it with any camera app, which opens the `fennec://send` link).
   Every recording not sent goes, one after another, with progress, and
   joins Fennec's Files queue to be transcribed. The phone and the computer
   must be on the same Wi-Fi network.

There is no pairing: each code works once, only for the Fennec that shows
it (its certificate is pinned in the code), and nothing secret stays on the
phone. A transfer that stops halfway resumes with the next code.

A quick settings tile and a home screen widget start a recording in one tap.

Each scan also brings Fennec's projects and templates, so new recordings
can be filed under them (projects are made and changed in Fennec). Under
**Settings** you choose the template new recordings get when their project
has none. A recording's template is the one picked for it, else its
project's default, else this phone's default, else Fennec's.

No shared Wi-Fi? Plug the phone into the computer with a USB cable and
choose **File transfer** on the phone. The app keeps a copy of each recording
not yet sent in `Download/Fennec Recorder` (a sidecar `.json` beside it names
title, project and template); Fennec offers to import them, then deletes the
copies. The app sees its copy gone and shows **Sent by USB**: Android lets an
app see only its own files, so that deletion is how the phone learns Fennec
has it. **Mark as sent to Fennec** and **Mark as not sent** on a recording
cover the cases it cannot tell (copied by hand, deleted by mistake). The
copies can be turned off in Settings; other apps on the phone can read them.

New installs record in high quality (48 kHz) and keep recordings after
they are sent; both can be changed in Settings.

## Building

```bash
source android/env.sh       # JDK 21, the Android SDK, emulator-5580
cd android
./gradlew :app:assembleDebug
```

A release build (`:app:assembleRelease`) is signed when `FENNEC_KEYSTORE`,
`FENNEC_KEYSTORE_PASSWORD`, `FENNEC_KEY_ALIAS` and `FENNEC_KEY_PASSWORD` are
set as Gradle properties or environment variables, and unsigned otherwise.
Tagged releases build it on GitHub (`.github/workflows/release.yml`) and
attach the signed APK next to the `.deb`.

## Tests

| Command | What it covers |
|---|---|
| `./gradlew :app:testDebugUnitTest` | Send links (and an older Fennec's pairing code told apart), certificate pinning, the single-use ticket and session, transfers against a fake Fennec (chunks, a lost answer, a damaged file, Fennec stopping midway and the next code resuming, a refused recording tried again), QR codes read by the phone's scanner, forgetting the old pairing and migrating recordings on upgrade, recovery after the app is killed, clean-up, the theme's colours against Fennec's CSS, and screenshots of every screen in light and dark |
| `./gradlew :app:recordRoborazziDebug` | Records new screenshot baselines (`app/src/test/screenshots`) after a deliberate UI change |
| `cargo test --test usb_import` (desktop) | USB import against a folder laid out as a phone shows it; `FENNEC_USB_PHONE=<folder>` imports a folder pulled from a real phone or the emulator (`adb pull "/sdcard/Download/Fennec Recorder"`) |
| `e2e/run-e2e.sh` | The real Fennec (headless, receiving) and the app in the emulator: record twice (once with a pause), open Fennec's send link, see both sent and Fennec's projects arrive. Then checks on the desktop that both arrived, decode to their full length, are dated and filed as on the phone, and are transcribed |

The end-to-end run needs the emulator (`pixel8-api36` on port 5580) and a
Wayland display or `weston` to start a headless one:

```bash
sg kvm -c "$ANDROID_HOME/emulator/emulator -avd pixel8-api36 -port 5580 -no-window -no-boot-anim -gpu swiftshader_indirect -no-snapshot-save" &
android/e2e/run-e2e.sh
```

## Fonts

IBM Plex Sans and IBM Plex Mono (`LICENSE-IBMPlex.txt`) and Source Serif 4
(`../data/fonts/LICENSE-SourceSerif4.md`), all under the SIL Open Font
License, as in the desktop app.
