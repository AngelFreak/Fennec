#!/usr/bin/env bash
# End to end: Fennec on this computer (the real window, headless) and Fennec
# Recorder in the emulator. The phone pairs through Fennec's pairing link,
# records twice, sends, sees both transcribed and unpairs; then this script
# checks what Fennec received.
#
#   android/e2e/run-e2e.sh            (emulator-5580 must be running: see env.sh)
#
# The link goes to the test base64-encoded: the device's shell would split it at "&".
# Needs: a Wayland display, or weston to start a headless one. Writes
# screenshots and Fennec's results to $OUT (default: a new temp folder).
set -euo pipefail
A=$(cd "$(dirname "$0")/.." && pwd); R=$(dirname "$A"); source "$A/env.sh"
OUT=${OUT:-$(mktemp -d -t fennec-e2e.XXXX)}; mkdir -p "$OUT"
PORT=${FENNEC_PHONE_E2E_PORT:-47131}
echo "results in $OUT"

[[ "$(adb get-state 2>/dev/null)" == device ]] || { echo "$ANDROID_SERIAL is not online" >&2; exit 1; }

PIDS=()
cleanup() { touch "$OUT/stop"; sleep 1; for p in "${PIDS[@]}"; do kill "$p" 2>/dev/null || true; done; }
trap cleanup EXIT

if [[ -z "${WAYLAND_DISPLAY:-}" ]]; then
  weston --backend=headless --width=1280 --height=800 --shell=kiosk --socket=wayland-fennec-e2e >"$OUT/weston.log" 2>&1 &
  PIDS+=($!); export WAYLAND_DISPLAY=wayland-fennec-e2e
  for _ in $(seq 50); do [[ -S "${XDG_RUNTIME_DIR:-/run/user/$(id -u)}/$WAYLAND_DISPLAY" ]] && break; sleep 0.1; done
fi
export GDK_BACKEND=wayland

(cd "$R" && cargo build --test phone_e2e -q)
rm -f "$OUT/pair-uri" "$OUT/results.json"
(cd "$R" && FENNEC_PHONE_E2E_DIR="$OUT" FENNEC_PHONE_E2E_PORT="$PORT" cargo test --test phone_e2e -q >"$OUT/desktop.log" 2>&1) &
PIDS+=($!)
for _ in $(seq 120); do [[ -s "$OUT/pair-uri" ]] && break; sleep 0.5; done
[[ -s "$OUT/pair-uri" ]] || { echo "Fennec did not start receiving:"; cat "$OUT/desktop.log"; exit 1; }
echo "pairing link: $(cat "$OUT/pair-uri")"

adb uninstall io.github.fennec.recorder >/dev/null 2>&1 || true
(cd "$A" && ./gradlew -q :app:connectedDebugAndroidTest \
  -Pandroid.injected.androidTest.leaveApksInstalledAfterRun=true \
  -Pandroid.testInstrumentationRunnerArguments.class=io.github.fennec.recorder.EndToEndTest \
  "-Pandroid.testInstrumentationRunnerArguments.pairUri=$(base64 -w0 "$OUT/pair-uri" | tr '+/' '-_' | tr -d '=')")
adb pull /sdcard/Android/data/io.github.fennec.recorder/files/e2e "$OUT/phone" >/dev/null

# Fennec's side: let it write its final state, then check it.
sleep 2
python3 - "$OUT/results.json" <<'PY'
import json, sys
r = json.load(open(sys.argv[1]))
recs = {x["title"]: x for x in r["recordings"]}
problems = []
def need(ok, what):
    if not ok: problems.append(what)
need(r["devices"] == 0, f"the phone is still paired in Fennec ({r['devices']} devices)")
need(set(recs) == {"E2E møde", "E2E med pause"}, f"recordings arrived: {sorted(recs)}")
for title, x in recs.items():
    need(x["state"] == "done", f"{title}: {x['state']} ({x['error']})")
    need(x["ext"] == "aac", f"{title}: sent as {x['ext']}")
    need(x["paragraphs"] == ["Hej fra telefonen."], f"{title}: transcript {x['paragraphs']}")
    need(x["created_at"] == x["recorded_at"], f"{title}: dated {x['created_at']}, recorded {x['recorded_at']}")
    need(x["decoded_ms"] is not None and abs(x["decoded_ms"] - x["duration_ms"]) < 1000,
         f"{title}: Fennec decoded {x['decoded_ms']} ms of a {x['duration_ms']} ms recording")
if "E2E møde" in recs:
    need(recs["E2E møde"]["project_id"] == 1, f"filed under project {recs['E2E møde']['project_id']}, not Kundemøder")
    need(5000 <= recs["E2E møde"]["duration_ms"] <= 9000, f"E2E møde lasted {recs['E2E møde']['duration_ms']} ms")
if "E2E med pause" in recs:
    # 4 s recorded around a 3 s pause: the pause is not in the recording.
    need(3000 <= recs["E2E med pause"]["duration_ms"] <= 6500, f"the paused one lasted {recs['E2E med pause']['duration_ms']} ms")
for p in problems: print("FAIL", p)
if problems: sys.exit(1)
print("ok   Fennec received both recordings, decoded them at full length and transcribed them; the phone unpaired")
PY
echo "screenshots: $OUT/phone and $OUT/*.png"
