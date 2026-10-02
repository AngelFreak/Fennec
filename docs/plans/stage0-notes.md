# Stage 0 notes: engine, models and measurements

Measured 2026-10-02 on the target laptop: Ryzen 5 7640U, Radeon 760M
(RADV), 30 GB RAM, Ubuntu 24.04, Wayland.

## Decisions

- **Engine:** whisper.cpp through `whisper-rs` 0.16, with the `vulkan` and
  `cuda` features. One `Transcriber` trait hides it. Models whisper.cpp
  cannot run (hviske-v6) use a Python helper behind the same trait (Stage 7b).
- **Voice detection:** Silero v6.2, through whisper-rs's `WhisperVadContext`
  (`ggml-silero-v6.2.0.bin`, 0.9 MB). It is used for file chunks and live
  utterances. If the file is missing, a loudness detector takes over.
- **Default model: Edda v0.1.** It ranks first among open models on the RyeAI
  Danish ASR leaderboard (9.21 mean WER), is Apache 2.0, and was the most
  accurate and the fastest here.
- **Conversion:** Edda is published only as a Hugging Face checkpoint.
  `scripts/convert_model.py` runs whisper.cpp's `convert-h5-to-ggml.py`.
  It fills in the vocabulary files Edda leaves out from
  `openai/whisper-large-v3-turbo`, then quantizes to q5_0. The result is
  574 MB (3.1 GB at f16).
- **Decode without timestamp tokens.** Edda and Røst were fine-tuned without
  them. Asking for timestamps garbled the first words of each clip, e.g.
  "ketchupapirakanske" for "Som i alle sydafrikanske". Paragraph times come
  from voice detection instead.

  | Model (50 FLEURS clips) | WER with timestamps | WER without | CER with | CER without |
  |---|---|---|---|---|
  | Edda | 18.7% | 7.9% | 12.1% | 3.2% |
  | Røst | 26.1% | 11.5% | 15.4% | 4.6% |

## Benchmark

`fennec-bench` on the first 200 clips of the Danish FLEURS test split
(Google, CC BY 4.0), with the Vulkan build. Speed was timed on 20 clips per
model, one model at a time.

| Model | Size | WER | CER | Load | RTF, GPU (Vulkan) | RTF, CPU (10 threads) |
|---|---|---|---|---|---|---|
| Edda v0.1 q5_0 | 574 MB | **8.8%** | **3.0%** | 0.6 s | **0.19** | 1.9 (10 clips) |
| Røst v3 q8_0 | 1.7 GB | 11.6% | 4.0% | 1.4 s | 0.28 | – |
| whisper tiny (reference) | 77 MB | 105% | 47% | 0.1 s | 0.04 | 0.09 |

RTF is processing time divided by audio length; below 1 keeps up with speech.

What the numbers mean for this laptop:
- **Live dictation needs the GPU build.** With Vulkan, a 5 s utterance
  takes about 1 s with Edda. On the CPU alone it takes about 10 s, so the
  text would fall further and further behind. Install with
  `make install FEATURES=vulkan`.
- **File transcription works on either.** One hour of audio takes about
  11 minutes on the GPU, or about two hours on the CPU.
- **Names are what both models get wrong most.** The worst clips are
  foreign names ("Lakkha Singh", "Cabo da Roca"). The vocabulary field in
  Settings (the initial prompt) is the lever for that.

## Found along the way

- Two whisper.cpp contexts running on one Vulkan device at once crash
  inside ggml. The app always uses one shared engine worker, so it is not
  affected. `fennec-bench` runs GPU models one after another.
- `glslc` (needed for the Vulkan build) can be unpacked from Ubuntu's
  `glslc` and `libshaderc1` packages without root; see the README.

## Not measured

- **Hviske v3 conversation:** not downloaded (1.1 GB at q5_0,
  non-commercial licence).
- **Hviske v6:** it runs through the Python helper, and `tests/sidecar.rs`
  covers the wiring with a stand-in model. The 2.8 GB model has not been
  downloaded, so `real_hviske_v6_transcribes_danish` is still ignored.
- **The full 930-clip split and the other leaderboard sets** (CoRal,
  Common Voice, FTSpeech) were not run.
