"""Convert a Hugging Face Whisper checkpoint to a quantized GGML model.

Usage: convert_model.py <hf_repo> <out_name> [quant]   (quant default q5_0)
Example: convert_model.py danish-foundation-models/edda-v0.1 edda-v0.1 q5_0
Writes ~/.local/share/fennec/models/<out_name>-<quant>.bin.

Needs a whisper.cpp checkout (WHISPER_CPP, default ~/dev/whisper.cpp) with
the whisper-quantize tool built in build-fennec/, and torch + transformers.
"""
import json, os, shutil, subprocess, sys, tempfile, urllib.request
from pathlib import Path
from huggingface_hub import hf_hub_download, snapshot_download

WHISPER_CPP = Path(os.environ.get("WHISPER_CPP", Path.home() / "dev/whisper.cpp"))
MODELS = Path.home() / ".local/share/fennec/models"
MEL_URL = "https://github.com/openai/whisper/raw/main/whisper/assets/mel_filters.npz"
# Base checkpoints whose tokenizer files fill in what fine-tunes leave out, by vocab size.
TOKENIZER_SOURCES = {51866: "openai/whisper-large-v3-turbo", 51865: "openai/whisper-large-v2"}


def main(repo: str, name: str, quant: str) -> None:
    src = Path(snapshot_download(repo))
    quantize = WHISPER_CPP / "build-fennec/bin/whisper-quantize"
    if not quantize.exists():
        sys.exit(f"missing {quantize}; build it with: cmake --build build-fennec --target whisper-quantize")
    MODELS.mkdir(parents=True, exist_ok=True)
    out = MODELS / f"{name}-{quant}.bin"
    with tempfile.TemporaryDirectory() as tmp:
        tmp = Path(tmp)
        model_dir = tmp / "model"
        model_dir.mkdir()
        for f in src.iterdir():
            (model_dir / f.name).symlink_to(f.resolve())
        fill_tokenizer(model_dir)
        assets = tmp / "openai" / "whisper" / "assets"
        assets.mkdir(parents=True)
        urllib.request.urlretrieve(MEL_URL, assets / "mel_filters.npz")
        run([sys.executable, str(WHISPER_CPP / "models/convert-h5-to-ggml.py"), str(model_dir), str(tmp / "openai"), str(tmp)])
        run([str(quantize), str(tmp / "ggml-model.bin"), str(out), quant])
    print(f"OK {out} ({out.stat().st_size / 1e6:.0f} MB)")


def fill_tokenizer(model_dir: Path) -> None:
    vocab_size = json.loads((model_dir / "config.json").read_text())["vocab_size"]
    missing = [f for f in ("vocab.json", "added_tokens.json") if not (model_dir / f).exists()]
    if not missing:
        return
    base = TOKENIZER_SOURCES.get(vocab_size)
    if base is None:
        sys.exit(f"{missing} missing and no known base tokenizer for vocab_size {vocab_size}")
    for f in missing:
        shutil.copy(hf_hub_download(base, f), model_dir / f)
    vocab = json.loads((model_dir / "vocab.json").read_text())
    added = json.loads((model_dir / "added_tokens.json").read_text())
    if len(vocab) + len(added) != vocab_size:
        sys.exit(f"tokenizer from {base} has {len(vocab) + len(added)} tokens, model expects {vocab_size}")
    print(f"tokenizer files {missing} taken from {base}")


def run(cmd: list) -> None:
    print("+", " ".join(cmd), flush=True)
    subprocess.run(cmd, check=True)


if __name__ == "__main__":
    if len(sys.argv) < 3:
        sys.exit(__doc__)
    main(sys.argv[1], sys.argv[2], sys.argv[3] if len(sys.argv) > 3 else "q5_0")
