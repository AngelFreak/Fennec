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
import os
import sys
import traceback


REPLIES = sys.stdout  # replies only; library output is sent to stderr


def send(obj):
    REPLIES.write(json.dumps(obj, ensure_ascii=False) + "\n")
    REPLIES.flush()


def pick_device():
    try:
        import torch
    except ImportError:
        return "cpu"
    return "cuda" if torch.cuda.is_available() else "cpu"


def cpu_has_bf16():
    """Zen 4 and recent Intel CPUs do bfloat16 in hardware; on them it is
    about twice as fast as float32 for hviske-v6, with the same accuracy.
    Elsewhere it is emulated and slower."""
    try:
        with open("/proc/cpuinfo") as f:
            flags = f.read()
    except OSError:
        return False
    return "avx512_bf16" in flags or "amx_bf16" in flags


def pick_dtype(device):
    """None leaves the model's own choice (bfloat16 on CUDA, float32 on CPU)."""
    if device != "cpu" or not cpu_has_bf16():
        return None
    try:
        import torch
    except ImportError:
        return None
    return torch.bfloat16


def transformers_compat():
    """hviske-v6's code was written for transformers 4, where
    `_tied_weights_keys` is a list. transformers 5 wants a mapping from each
    tied weight to its source; an LM head is tied to the token embeddings."""
    try:
        import transformers
        from transformers import modeling_utils
    except ImportError:
        return
    if int(transformers.__version__.split(".")[0]) < 5:
        return
    original = modeling_utils.PreTrainedModel.post_init

    def post_init(self):
        keys = getattr(type(self), "_tied_weights_keys", None)
        if isinstance(keys, list):
            mapping = {}
            for k in keys:
                if k.endswith("lm_head.weight"):
                    mapping[k] = k[: -len("lm_head.weight")] + "model.embed_tokens.weight"
            type(self)._tied_weights_keys = mapping
        return original(self)

    modeling_utils.PreTrainedModel.post_init = post_init

    # The model's tokenizer sits next to its custom config; without this,
    # transformers stops to ask whether to trust that code.
    tokenizer = transformers.AutoTokenizer
    from_pretrained = tokenizer.from_pretrained.__func__

    def trusted(cls, *args, **kw):
        kw.setdefault("trust_remote_code", True)
        return from_pretrained(cls, *args, **kw)

    tokenizer.from_pretrained = classmethod(trusted)


def rebuild_rotary(model):
    """transformers 5 builds models on the meta device and only fills what
    the checkpoint holds, so hviske-v6's frame rotary tables (non-persistent
    buffers) come back uninitialised and every output turns to NaN."""
    for module in model.modules():
        if type(module).__name__ == "FrameRotary":
            cos, sin = module._build(module.cos_cached.shape[2])
            device = next(model.parameters()).device
            module.cos_cached, module.sin_cached = cos.to(device), sin.to(device)


def load(model_dir):
    transformers_compat()
    sys.path.insert(0, model_dir)
    from processing_whisper_qwen import HviskeASR  # the model's own code

    device = pick_device()
    asr = HviskeASR.from_pretrained(model_dir, device=device, dtype=pick_dtype(device))
    if hasattr(asr, "model"):
        rebuild_rotary(asr.model)
    return asr, device


def main():
    if len(sys.argv) != 2:
        send({"error": "usage: hviske_sidecar.py <model-dir>"})
        return 2
    # Requests arrive on stdin and replies leave on stdout; nothing the
    # libraries print or ask may touch either.
    stdin = sys.stdin.buffer
    sys.stdin = open(os.devnull)
    sys.stdout = sys.stderr
    try:
        asr, device = load(sys.argv[1])
    except Exception as e:  # report why, so Fennec can show it
        traceback.print_exc(file=sys.stderr)
        send({"error": str(e)})
        return 1
    import numpy as np

    send({"ready": True, "device": device})
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
