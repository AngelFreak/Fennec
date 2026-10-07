# Fennec Recorder

The Android companion to Fennec: record meetings and interviews on your phone,
and the audio goes to Fennec on your computer over your own network, where it
is transcribed with the models you already have. Nothing passes through the
internet. Android 10 or newer.

The design and the phone protocol are in
[`docs/plans/2026-10-06-android-recorder.md`](../docs/plans/2026-10-06-android-recorder.md).

## Using it

1. In Fennec: **Settings → Phone → Pair a phone**. A QR code appears.
2. In Fennec Recorder: point the camera at it (or choose *No camera?* and
   type the address and code Fennec shows).
3. Both screens show the same 8-digit code. Press **Allow** in Fennec.
4. Record. Each recording is sent when the phone is on your network (only on
   Wi-Fi without a data limit, unless you change that in Settings), joins
   Fennec's Files queue and is transcribed; the phone shows when it is done.

A quick settings tile and a home screen widget start a recording in one tap.

Under **Settings** you can add projects and change their name, colour and
default template (the changes go to Fennec and show in its sidebar; deleting
stays on the computer), and choose the template new recordings get when their
project has none. A recording's template is the one picked for it, else its
project's default, else this phone's default, else Fennec's.

New installs record in high quality (48 kHz) and keep recordings after
Fennec has transcribed them; both can be changed in Settings.

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
| `./gradlew :app:testDebugUnitTest` | Pairing links and codes (the code is checked against the desktop's), certificate pinning, uploads against a fake Fennec (chunks, a lost answer, a damaged file, unpairing), the desktop's QR code read by the phone's scanner, recovery after the app is killed, clean-up, the theme's colours against Fennec's CSS, and screenshots of every screen in light and dark |
| `./gradlew :app:recordRoborazziDebug` | Records new screenshot baselines (`app/src/test/screenshots`) after a deliberate UI change |
| `e2e/run-e2e.sh` | The real Fennec (headless) and the app in the emulator: pair through Fennec's link, record twice (once with a pause), send, see both transcribed, unpair. Then checks on the desktop that both arrived, decode to their full length, are dated and filed as on the phone, and that the phone is unpaired |

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
