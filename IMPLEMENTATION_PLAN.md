# Fennec implementation plan

Design: `docs/plans/2026-10-02-fennec-design.md`. Each stage: test first,
`cargo build`, `cargo test`, `cargo clippy -- -D warnings`, `cargo fmt`,
commit. Integration tests drive the real wiring.

## Stage 0: Scaffold and engine spike
**Goal**: Cargo project, whisper-rs transcribing Danish audio on CPU; Edda converted to GGML; VAD source decided.
**Success Criteria**: `fennec-bench <wav>` prints Danish text + timing; tiny test model downloads for tests; decision notes in `docs/plans/stage0-notes.md`.
**Tests**: engine transcribes a known WAV with the tiny model (non-empty text, segments ordered).
**Status**: In Progress — engine, bench tool, WER/CER and conversion scripts written; waiting on FLEURS + model downloads for the integration test and benchmark. VAD: whisper-rs 0.16 exposes Silero VAD (`WhisperVadContext`).

## Stage 1: Store and domain
**Goal**: SQLite schema + migrations; projects, documents, tags, paragraphs, FTS search, summaries, action items.
**Success Criteria**: CRUD + tag filter + FTS search through the public API.
**Tests**: per-query unit tests; migration from empty DB; FTS finds Danish text with æøå.
**Status**: Complete

## Stage 2: Templates and export
**Goal**: TOML templates (fields, heading, footer, logo, font, `{summary}` slot); TXT, DOCX, PDF writers; combined project export.
**Success Criteria**: export a stored document to all three formats; required-field validation.
**Tests**: parse/validate templates; DOCX XML contains fields + text; PDF text extractable; project export ordering.
**Status**: Complete — PDF via cairo + pango (two-pass for "Side N af M"), DOCX as hand-written OOXML; logos must be PNG.

## Stage 3: Audio, VAD and file ingest
**Goal**: decode (symphonia + ffmpeg fallback), resample, VAD chunking, ingest job with progress into the store.
**Success Criteria**: a WAV/MP3 becomes timestamped paragraphs in the DB.
**Tests**: resampler, VAD on synthetic tone/silence; integration: file → engine → store → DOCX.
**Status**: Complete — symphonia with ffmpeg fallback, rubato FFT resampling, Silero VAD via whisper-rs, paragraph builder shared with live dictation.

## Stage 4: Live dictation pipeline
**Goal**: cpal capture, utterance builder, partials, voice commands, low-confidence spans, disk buffer, engine priority queue.
**Success Criteria**: fake audio source → events → paragraphs, commands applied.
**Tests**: utterance boundaries, force-cut, partial dropping when busy, command matching; integration through the real pipeline.
**Status**: Complete — frame-level Silero over a sliding window; engine worker shared by live and file jobs (finals > file > previews); mic via cpal on its own thread; untested on real hardware until the GTK app runs (Stage 5).

## Stage 5: GTK shell — window, sidebar, dictation screen
**Goal**: main window per mockup: header, sidebar (nav, projects, tags, settings), editor with tags, record dock, inspector fields, CSS.
**Success Criteria**: app launches, dictation writes into the editor, autosave to DB.
**Tests**: UI smoke test builds window; editor-model unit tests (partial replace, paragraph mapping).
**Status**: Complete — tests/ui.rs drives the real window with a scripted engine and synthetic audio (22 checks); optional PNG screenshots via FENNEC_SCREENSHOT_DIR. Files/Templates/Project/Settings pages are placeholders until Stage 6–7.

## Stage 6: Files, export, templates, project screens
**Goal**: file queue + player/timeline, export dialog with preview, template editor, project view (docs/filter/search).
**Success Criteria**: every mockup screen except AI reachable and working.
**Tests**: UI smoke per screen; integration: import file via UI action → paragraphs shown.
**Status**: Complete — Files, Export, Templates editor (with live preview and validation) and Project view (search, tag filter, settings, local-only, combined export); documents move between projects and get tags from the Dictate screen. UI test covers each screen.

## Stage 7: Models and compute
**Goal**: model catalog, download + convert + checksum, backend selection (auto/CUDA/Vulkan/CPU) with fallback, speed test; Settings speech/dictation/storage sections.
**Success Criteria**: switch model/backend from Settings; Vulkan build works on this laptop.
**Tests**: catalog/path logic; fallback chain with a failing backend stub.
**Status**: Complete except the Vulkan build — catalog (Edda, Hviske v3, Røst v3), resumable downloads, embedded converter, custom models, backend options with reasons, GPU→CPU fallback, speed test, Dictation and Storage settings, all saved to settings.toml. Vulkan needs `glslc` (not installed here, no sudo); build with `--features vulkan` once it is.

## Stage 7b (optional, last): Hviske v6 sidecar engine
**Goal**: second `Transcriber` running syvai/hviske-v6 (Whisper encoder + Qwen3 decoder, custom transformers code) in a Python helper process; selectable in Settings and in fennec-bench.
**Success Criteria**: v6 transcribes file chunks and dictation utterances through the same pipeline; timestamps come from VAD chunks; no low-confidence spans.
**Tests**: protocol unit tests with a fake helper script; integration run against the real model behind `#[ignore]` (needs the 2.8 GB download).
**Status**: Deferred — leaderboard (RyeAI, same harness for all) puts Edda first among open models (9.21 mean WER vs 10.36 for v6); do after Stage 9.

## Stage 8: AI providers and actions
**Goal**: Anthropic + OpenAI-compatible providers, keyring, privacy gate, summary/clean-up/action items/ask/fields, Settings AI sections, UI (AI menu, tabs, diff view, Ask tab).
**Success Criteria**: each action works against mock servers and a real provider when configured.
**Tests**: mock-server integration per protocol and action; AI-off makes zero requests; local-only rejects cloud; citation validation.
**Status**: Not Started

## Stage 9: Packaging and docs
**Goal**: Makefile install, .desktop, icon, cargo-deb, README.
**Success Criteria**: `make install` gives a launchable app.
**Tests**: release build; desktop file validates.
**Status**: Not Started
