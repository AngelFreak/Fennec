# Fennec — design

Local Danish dictation and transcription for Linux. Rust, GTK 4.
Status: validated with the user on 2026-10-02. UI mockup:
https://claude.ai/artifact/M8L99e5fdX9cqE68hvHAvm

## Goals

- **Live dictation** into an in-app editor on this laptop (Ryzen 5 7640U,
  Radeon 760M, 30 GB RAM, Wayland/PipeWire).
- **File ingest**: audio/video files transcribed with timestamps.
- **Output**: plain text, DOCX, PDF, rendered through **user-definable report
  templates** (header fields + body).
- **Grouping**: projects (one per document) and tags (many per document).
- **Optional AI** (off by default): summaries, clean-up, action items, ask the
  project, field suggestions — via Claude, ChatGPT, or local/network LLMs.
- **Personal use.** Non-commercial model licenses (Hviske) are acceptable.

Non-goals for v1: speaker diarization (data model keeps a `speaker` slot),
system-wide dictation into other apps, system-wide hotkey (needs the
GlobalShortcuts portal; later).

## UI

English UI; document content is Danish. Screens (see mockup):

1. **Dictate** — editor (transcript / summary / actions tabs), record dock
   (button, timer, level meter, status, Ctrl+Space), inspector with template
   fields, low-confidence review, AI field suggestions.
2. **Files** — drop zone + queue, transcript with timestamp gutter that fills
   in while transcribing, player with played/transcribed/pending timeline.
3. **Export** — TXT / DOCX / PDF, template, content options, preview,
   blocked while required fields are empty.
4. **Templates** — fields (label, type, default placeholder, required),
   heading, logo, body font, footer; stored as TOML.
5. **Project view** — documents list with tag filters and search, Actions
   roll-up, Ask (Q&A with citations), project export.
6. **Settings** — speech model, dictation, AI providers, AI defaults,
   privacy, storage.
7. **Clean-up** — per-paragraph word diff, accept / keep original / undo.

Sidebar: Dictate, Files, Templates; Projects (with counts) and Tags; Settings
pinned at the bottom.

## Stack

- **GTK 4 + libadwaita** (`gtk4` 0.11, `libadwaita` 0.9), custom CSS for the
  mockup look (IBM Plex Sans, Source Serif 4, IBM Plex Mono; accent `#C2410C`).
  Editor is `GtkTextView` with text tags (partial = grey italic, unsure =
  dotted underline). Waveform and level meter are `GtkDrawingArea`.
- **Speech**: whisper.cpp via `whisper-rs`. Models in GGML format.
- **Storage**: SQLite (`rusqlite`, bundled) with FTS5.
- **Audio**: `cpal` (ALSA → PipeWire) for capture, `symphonia` for decoding,
  `ffmpeg` subprocess fallback for video containers, `rubato` resampling.
- **AI**: blocking `reqwest` (rustls) on worker threads, like model
  downloads; streaming reads server-sent events line by line. (Changed from
  a tokio runtime in Stage 8: no async code was needed anywhere else.)
- **Exports**: DOCX written directly as OOXML (zip + XML); PDF via a
  layout-capable crate (decided in Stage 5).

## Models

| Model | Base | License | Role |
|---|---|---|---|
| Edda v0.1 (Alexandra Inst.) | whisper-large-v3-turbo, 0.8B | Apache 2.0 | default |
| Hviske v3 conversation (syv.ai) | Røst / whisper-large-v3, 1.5B | OpenRAIL, non-commercial | meetings |
| Røst v1 Whisper 1.5B (CoRal) | whisper-large-v3, 1.5B | OpenRAIL | alternative |

None publish GGML; `scripts/convert-model.sh` converts Hugging Face weights
with whisper.cpp's converter, then quantizes (q5_0). Models live in
`~/.local/share/fennec/models/`. One model loaded at a time.

## Architecture

```
src/
  main.rs            entry; lib.rs holds the core (no GTK imports)
  audio/             capture, decode, resample to 16 kHz mono f32
  vad.rs             frame VAD + utterance builder
  engine/            trait Transcriber; whisper.rs (whisper-rs)
  live.rs            dictation pipeline: VAD → utterance → engine → events
  ingest.rs          file queue: decode → VAD chunks → engine → progress
  commands.rs        Danish voice commands
  models.rs          catalog, paths, checksum, backend selection
  store/             SQLite schema, migrations, queries, FTS
  template/          TOML templates: parse, validate, render
  export/            txt, docx, pdf, combined project export
  ai/                provider trait, anthropic.rs, openai_compat.rs,
                     actions/ (summary, cleanup, actions, ask, fields),
                     privacy.rs
  config.rs          settings (TOML in ~/.config/fennec/settings.toml)
  ui/                GTK widgets + CSS
```

**Threads.** GTK main thread never blocks. One **engine worker** thread owns
the loaded model and takes jobs from a priority channel (live final >
file chunk > live partial; partials are dropped when busy, never queued). An
**audio** thread feeds a ring buffer. Each AI request runs on its own
worker thread with its own store connection. Results reach the UI as `Event`s over `async_channel`, consumed via
`glib::spawn_future_local`.

## Data model (SQLite at `~/.local/share/fennec/fennec.db`)

- `projects(id, name, color, default_template, local_only)`
- `documents(id, project_id NULL, title, template_id, fields_json,
  source 'dictation'|'file', audio_path NULL, created_at, updated_at)`
- `tags(id, name UNIQUE)`, `document_tags(document_id, tag_id)`
- `paragraphs(id, document_id, ord, text, start_ms NULL, end_ms NULL,
  low_conf_json, speaker NULL)` + FTS5 `paragraphs_fts`
- `summaries(id, document_id NULL, project_id NULL, prompt_id, provider,
  model, text, created_at, include_in_export)`
- `action_items(id, document_id, paragraph_id NULL, what, who NULL,
  due NULL, done, created_at)`

The **paragraph is the unit**: engine output becomes timestamped paragraphs;
edits keep the paragraph's time range; splitting divides it proportionally.
Autosave 1 s after typing stops and on every committed utterance. SQLite WAL.

Templates and summary prompts are TOML files in `~/.config/fennec/templates/`
and `~/.config/fennec/prompts/`. Invalid files show an error on their card.

## Live dictation

- Capture 48 kHz → resample 16 kHz mono → 30 ms frames → VAD.
- Utterance starts after ~100 ms speech (200 ms pre-roll kept), ends after a
  configurable pause (default 0.6 s), force-cut at 25 s at the quietest point
  of the last 2 s.
- Partial preview after 0.7 s of speech, then every 1 s, only when the engine
  is idle and not in a pause; utterances up to 3 s get a quick pass first, so
  commands act on it alone.
- Per-utterance params: language `da`, no translation, no initial prompt
  (any prompt wrecks the Danish fine-tunes: Edda's FLEURS WER went from 7.8%
  to 87.7% with a three-word vocabulary as the prompt; the vocabulary corrects
  near-misses in the text instead); tokens below a probability
  threshold become low-confidence spans.
- Voice commands fire only when the whole normalized utterance matches
  ("nyt afsnit"/"ny paragraf", "ny linje", "slet sidste sætning",
  "stop diktat"/"stop optagelse", and spoken punctuation such as "punktum").
- Falling behind is shown ("behind by 4 s"); audio is never dropped.
- Raw audio is buffered to disk during dictation (crash safety; optional keep).
- VAD source decided in Stage 0: whisper.cpp's Silero VAD through
  `whisper-rs` if exposed, otherwise an energy + zero-crossing VAD in Rust
  (no ONNX runtime dependency).

## Compute backends

CPU (always), Vulkan (AMD/Intel/NVIDIA), CUDA (NVIDIA). "Automatic" tries
CUDA → Vulkan → CPU with a test transcription at startup and falls back to CPU
with a banner on failure. Built with cargo features `vulkan` and `cuda`
(two packages if runtime-loaded ggml backends are not usable through
`whisper-rs`). A speed test reports latency per sentence; CPU-only machines
get suggestions (q4 model, longer pause, file mode).

## AI (optional, off by default)

**Providers** (presets): Claude (native Anthropic Messages API,
`claude-opus-5-5` default, streaming, refusal handling), ChatGPT (OpenAI API),
Ollama (this computer), llama.cpp / LM Studio / vLLM, network server, other
OpenAI-compatible cloud. Two protocol implementations behind one trait:

```rust
trait LlmProvider {
    fn stream_text(&self, req: TextRequest) -> Stream<Result<String>>;
    fn structured(&self, req: StructuredRequest) -> Result<serde_json::Value>;
}
```

Structured output uses each API's JSON-schema mode; fallback is prompt-only
JSON, validate, retry once, then a clear error. API keys in the Secret
Service keyring. Model lists fetched live (`/v1/models`).

**Privacy.** Each provider is labelled local / network / cloud. Projects can
be **local only** (cloud providers hidden and rejected in code). First cloud
send of a document needs confirmation naming the destination. Nothing runs
automatically. Audio never leaves the machine. While dictation is live,
AI jobs on a local-this-machine provider are queued.

**Actions.**
- *Summary* (document or project, oldest first) — streamed, editable,
  labelled AI-generated with provider/model, optional `{summary}` slot in
  templates.
- *Clean up* — one result per paragraph, word-level diff, accept / keep /
  undo; original retained.
- *Action items* — structured `[{what, who?, due?, paragraph_id}]`,
  relative dates resolved against the document date; project roll-up.
- *Ask the project* — paragraphs tagged `[d12:p4]`, model must cite;
  unknown citation IDs dropped; whole project if it fits the context,
  otherwise FTS5 top paragraphs.
- *Fill fields* — suggestions shown as ghost values; never overwrite user
  values.
- Long inputs exceeding the provider's context are chunked by paragraph
  (map-reduce).

## Error handling

Fail visibly with context; never swallow. Mic missing/denied → dock shows the
cause and a Settings link. Model missing or checksum mismatch → onboarding
card. GPU init failure → CPU fallback + banner. File decode failure → that
queue item shows the error; others continue. AI: network, 401, refusal,
context too long, invalid JSON → message on the action, no partial writes.
Corrupt template TOML → error on the template card. All background errors are
logged (`tracing`) with document/job IDs.

## Testing

- **Unit**: VAD/utterance builder on synthetic audio, voice commands,
  template parse/render, txt/docx/pdf writers (parse output back), store
  queries and migrations, AI request building and response validation with
  recorded fixtures, citation validation, privacy gate.
- **Integration (required)** through the real wiring:
  - ingest: WAV file → decode → VAD → whisper (tiny multilingual GGML in
    tests, Edda locally behind `#[ignore]`) → store → export DOCX → assert
    text present.
  - live: fake audio source feeding the real live pipeline → events →
    editor-model buffer → paragraphs in the store.
  - AI: local mock HTTP servers speaking the Anthropic and OpenAI protocols,
    driving each action end to end; a test proves no request is made when AI
    is off and that local-only projects reject cloud providers.
  - UI: GTK smoke test (harness = false) building the main window.

## Delivery

Makefile `install` to `~/.local`, `.desktop` file, `.deb` via `cargo-deb`,
following MultiSignal.
