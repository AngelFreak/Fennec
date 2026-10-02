# Fennec

Danish dictation and transcription that runs on your own computer.

- **Dictate** into Fennec's editor. Text appears when you pause. Spoken
  commands: *nyt afsnit*, *ny linje*, *slet sidste sætning*, *stop diktat*.
- **Transcribe files**: audio or video in, timestamped paragraphs out, with a
  player to check the result.
- **Report templates**: header fields plus body, edited in the app (TOML
  under `~/.config/fennec/templates`).
- **Export** to TXT, DOCX or PDF, one document or a whole project.
- **Projects and tags** group documents, for example every meeting with one
  vendor. Projects can be searched and exported together.
- **Optional AI**, off by default: summaries, clean-up shown as a diff, action
  items, questions answered from a project with sources, and suggested field
  values. Works with Claude, ChatGPT, or a model on this computer or your
  network (Ollama, llama.cpp, LM Studio, vLLM).

The interface is in English; the transcripts and AI answers are in Danish.

## Install

Fennec needs GTK 4.14+ and libadwaita 1.5+ (Ubuntu 24.04 or newer), Rust
1.88+, and the GTK development headers:

```sh
sudo apt install libgtk-4-dev libadwaita-1-dev libasound2-dev cmake clang
make install                 # into ~/.local (no root needed)
```

To use the GPU, build with a backend:

```sh
make install FEATURES=vulkan # AMD/Intel GPUs; needs glslc and libvulkan-dev
make install FEATURES=cuda   # NVIDIA; needs the CUDA toolkit
```

Without root, `glslc` can be unpacked from Ubuntu's packages:
`apt-get download glslc libshaderc1`, `dpkg -x` each into a folder, then put
its `usr/bin` on `PATH` and its `usr/lib/x86_64-linux-gnu` on
`LD_LIBRARY_PATH`. On a Radeon 760M the Vulkan build runs about 2.4× faster
than the CPU build.

Settings → Speech model shows which backends this build and computer can
use. If the GPU fails to start, Fennec falls back to the CPU.

For a Debian package, run `make deb`, then
`sudo apt install ./target/debian/fennec_*.deb`.

Optional runtime extras:
- `ffmpeg`, for video and other formats Fennec cannot decode itself.
- GStreamer good plugins, for playback in Files.
- A Secret Service keyring (GNOME Keyring or KWallet), for AI API keys.

## First run: pick a speech model

Open **Settings → Speech model** and install a model. The voice detector
(Silero, about 1 MB) is downloaded along with the first model.

| Model | Publisher | Licence | Size | Mean WER* |
|---|---|---|---|---|
| **Edda v0.1** (default) | Alexandra Institute | Apache 2.0 | ~550 MB (q5_0) | 9.2 % |
| Hviske v3 conversation | syv.ai | OpenRAIL, non-commercial | ~1.1 GB (q5_0) | – |
| Røst v3 Whisper 1.5B | CoRal project | OpenRAIL | 1.7 GB (q8_0) | 13.7 % |
| Hviske v6 | syv.ai | CC BY-NC 4.0 | 2.8 GB | 10.2 % |

\* From the [Danish ASR leaderboard](https://huggingface.co/spaces/RyeAI/danish-asr-leaderboard).

Røst v3 downloads as a ready-made GGML file. Edda and Hviske are published
only as Hugging Face checkpoints, so Fennec converts them, which needs:
- Python with `torch`, `transformers` and `huggingface_hub` (set
  `FENNEC_PYTHON` to use a virtualenv's Python).
- A [whisper.cpp](https://github.com/ggml-org/whisper.cpp) checkout (set
  `WHISPER_CPP`; the default is `~/dev/whisper.cpp`) with `whisper-quantize`
  built in `build-fennec/`.

Hviske v6 is not a Whisper model that whisper.cpp can run (it pairs a Whisper
encoder with a Qwen3 decoder). Fennec runs it in a small Python helper, which
needs Python with `torch` and `transformers`. It is slower than the others on
a CPU and gives no word confidences.

You can also add any whisper.cpp GGML file with **Add custom GGML model…**.
**Run speed test** measures how long a sentence takes on your machine.

## AI (optional)

AI is off until you switch it on in **Settings → AI providers**. Nothing is
sent anywhere until you start an action yourself. Audio never leaves the
computer.

- API keys are stored in the desktop keyring, not in `settings.toml`.
- The first time a document or project goes to a **cloud** provider, Fennec
  asks and names the destination.
- Projects marked **Local only** never go to a cloud provider.
- Jobs on a model running on this computer wait while you dictate.
- Every AI result is labelled with the provider and model that wrote it.
  Clean-up never changes text until you accept it.

## Where things live

| What | Where |
|---|---|
| Settings | `~/.config/fennec/settings.toml` |
| Templates | `~/.config/fennec/templates/*.toml` |
| Documents (SQLite) | `~/.local/share/fennec/fennec.db` |
| Models | `~/.local/share/fennec/models/` |
| Dictation audio | `~/.local/share/fennec/audio/` |
| Exports | `~/Documents/Fennec/` |

## Development

```sh
cargo test             # unit and integration tests, plus the GTK UI test
cargo clippy --all-targets
```

- The UI test (`tests/ui.rs`) opens the real window. It drives the app with a
  scripted engine and synthetic audio, and the AI actions against local mock
  servers, so it needs a display.
- Engine tests use `tests/fixtures/models/`, filled by
  `python scripts/fetch_models.py tiny vad`.
- `FENNEC_SCREENSHOT_DIR=dir cargo test --test ui` saves screenshots of each
  screen.

`fennec-bench` compares models on the same audio. It runs them in parallel
for accuracy, then one at a time for speed:

```sh
python scripts/prepare_fleurs.py      # Danish FLEURS test split → manifest.tsv
fennec-bench --manifest ~/.cache/fennec/fleurs-da/manifest.tsv --limit 200 \
    ~/.local/share/fennec/models/edda-v0.1-q5_0.bin ~/.local/share/fennec/models/roest-v3-q8_0.bin
```

## Credits

- Speech recognition: [whisper.cpp](https://github.com/ggml-org/whisper.cpp)
  (MIT) through [whisper-rs](https://github.com/tazz4843/whisper-rs).
- The speed-test clip and test fixture come from
  [FLEURS](https://huggingface.co/datasets/google/fleurs) (Google, CC BY 4.0).
- The models are by their publishers under the licences above. The Hviske v3
  and Hviske v6 licences do not allow commercial use.
