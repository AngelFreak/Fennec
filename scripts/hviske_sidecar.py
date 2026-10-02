"""Speech-to-text helper for models whisper.cpp cannot run (hviske-v6).

Fennec starts this with a model directory and talks to it over stdin/stdout:

  helper -> {"ready": true, "device": "cpu"}          once the model is loaded
            {"error": "..."}                           if loading failed, then exits
  Fennec -> {"id": 1, "samples": N, "punctuated": true}\n  then N little-endian f32
  helper -> {"id": 1, "text": "..."} or {"id": 1, "error": "..."}

Audio is 16 kHz mono, at most 30 s per request (Fennec cuts at pauses).
Logs go to stderr. Needs torch, transformers and numpy.
"""
import json
import sys
import traceback


def send(obj):
    sys.stdout.write(json.dumps(obj, ensure_ascii=False) + "\n")
    sys.stdout.flush()


def pick_device():
    try:
        import torch
    except ImportError:
        return "cpu"
    return "cuda" if torch.cuda.is_available() else "cpu"


def load(model_dir):
    sys.path.insert(0, model_dir)
    from processing_whisper_qwen import HviskeASR  # the model's own code

    device = pick_device()
    asr = HviskeASR.from_pretrained(model_dir, device=device)
    return asr, device


def main():
    if len(sys.argv) != 2:
        send({"error": "usage: hviske_sidecar.py <model-dir>"})
        return 2
    try:
        asr, device = load(sys.argv[1])
    except Exception as e:  # report why, so Fennec can show it
        traceback.print_exc(file=sys.stderr)
        send({"error": f"could not load {sys.argv[1]}: {e}"})
        return 1
    import numpy as np

    send({"ready": True, "device": device})
    stdin = sys.stdin.buffer
    while True:
        header = stdin.readline()
        if not header:
            return 0
        req = json.loads(header)
        raw = stdin.read(4 * int(req["samples"]))
        try:
            wav = np.frombuffer(raw, dtype="<f4")
            punctuated = req.get("punctuated", True)
            texts = asr.transcribe_batch([wav], cased=punctuated, punctuated=punctuated)
            send({"id": req["id"], "text": texts[0]})
        except Exception as e:
            traceback.print_exc(file=sys.stderr)
            send({"id": req["id"], "error": str(e)})


if __name__ == "__main__":
    sys.exit(main())
