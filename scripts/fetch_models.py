"""Download Danish Whisper checkpoints and the test assets Fennec needs.

Usage: fetch_models.py [name ...]   (default: all)
Raw checkpoints go to the Hugging Face cache; ready GGML files go to
~/.local/share/fennec/models. Each item reports OK or the error and the
script exits non-zero if any item failed.
"""
import os, sys, shutil, urllib.request
from pathlib import Path
from huggingface_hub import hf_hub_download, snapshot_download

MODELS = Path.home() / ".local/share/fennec/models"
FIXTURES = Path(__file__).resolve().parent.parent / "tests/fixtures/models"

def edda():       return snapshot_download("danish-foundation-models/edda-v0.1")
def hviske():     return snapshot_download("syvai/hviske-v3-conversation")
def hviske6():    return snapshot_download("syvai/hviske-v6", local_dir=str(MODELS / "hviske-v6"))  # run by the Python helper
def roest():
    p = hf_hub_download("alfanova/roest-v3-whisper-ggml", "roest-v3-q8_0.bin")
    shutil.copy(p, MODELS / "roest-v3-q8_0.bin"); return str(MODELS / "roest-v3-q8_0.bin")
def tiny():
    p = hf_hub_download("ggerganov/whisper.cpp", "ggml-tiny.bin")
    shutil.copy(p, FIXTURES / "ggml-tiny.bin"); return str(FIXTURES / "ggml-tiny.bin")
def vad():
    p = hf_hub_download("ggml-org/whisper-vad", "ggml-silero-v6.2.0.bin")
    shutil.copy(p, FIXTURES / "ggml-silero-v6.2.0.bin")  # tests use the same model
    shutil.copy(p, MODELS / "ggml-silero-v6.2.0.bin"); return str(MODELS / "ggml-silero-v6.2.0.bin")
def fleurs():
    return hf_hub_download("google/fleurs", "data/da_dk/test.tsv", repo_type="dataset") + " + " + \
           hf_hub_download("google/fleurs", "data/da_dk/audio/test.tar.gz", repo_type="dataset")

ALL = {"tiny": tiny, "vad": vad, "fleurs": fleurs, "roest": roest, "edda": edda, "hviske": hviske, "hviske6": hviske6}
names = sys.argv[1:] or list(ALL)
failed = 0
for n in names:
    try:
        print(f"[{n}] OK {ALL[n]()}", flush=True)
    except Exception as e:
        failed += 1
        print(f"[{n}] FAILED {type(e).__name__}: {e}", flush=True)
sys.exit(1 if failed else 0)
