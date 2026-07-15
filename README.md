# Voice Flow

A lightweight, open-source Tauri + Rust app for system-wide, real-time voice input. It deliberately does not replace or bundle itself with an input method: keep the keyboard and typing habits you already use, hold a shortcut, watch the transcript appear, and release to insert at the active cursor.

The interface uses a quiet, warm, writing-focused visual system rather than a high-tech dashboard. The floating overlay contains only recognized text, accepts no input, and never takes focus from the app receiving text.

## MVP scope

- macOS runtime support today; Linux-facing boundaries are isolated for a later implementation.
- VolcEngine WebSocket V3 bidirectional streaming ASR.
- One locally stored Secret Key for VolcEngine authentication.
- Global hold-to-talk and toggle shortcuts using any supported single key or key chord, including left/right modifier distinction.
- Manual microphone selection, with a system-default option.
- Live, click-through transcript-only overlay with no controls or decorative chrome.
- Raw streaming ASR output with VolcEngine punctuation and inverse-text normalization; no LLM polish in the MVP.
- Automatic clipboard + paste insertion when dictation ends.
- Credentials stored locally in the Tauri app config directory, never in this repository.

## Run

Prerequisites: Rust, Node.js, npm, Xcode command-line tools, and a VolcEngine Speech account.

```bash
npm install
npm run tauri dev
```

On first use, macOS asks for microphone access. Global side-specific shortcut detection and automatic insertion also need Accessibility permission for Voice Flow (or the terminal during development) under **System Settings → Privacy & Security → Accessibility**.

## Credentials and settings

Enter the VolcEngine **Secret Key** once. It is stored only in the local settings file. The Secret Key, selected microphone, shortcut, interaction mode, and insertion preference are saved automatically whenever they change.

Voice Flow uses the fixed ASR endpoint `wss://openspeech.bytedance.com/api/v3/sauc/bigmodel` and resource ID `volc.seedasr.sauc.duration`.

## Architecture

- `src-tauri/src/audio.rs`: microphone capture and 16 kHz mono PCM resampling.
- `src-tauri/src/asr/`: VolcEngine transport and binary protocol framing.
- `src-tauri/src/controller.rs`: dictation lifecycle and UI events.
- `src-tauri/src/shortcut.rs`: arbitrary key/chord polling with left/right modifier distinction.
- `src-tauri/src/logging.rs`: identical stdout and fixed-file tracing output.
- `src-tauri/src/platform/`: active-cursor insertion boundary; macOS is implemented, Linux is intentionally isolated.
- `src-tauri/src/config.rs`: local settings and validation.
- `src/`: framework-free TypeScript UI for the settings window and transcript-only overlay.

Provider endpoint/resource policy is owned by the backend. User choices such as microphone, interaction mode, shortcut, and insertion behavior live in the automatically persisted settings model.

Runtime logs are written to stdout and `~/Library/Logs/dev.voiceflow.desktop/voice-flow.log` with identical content. Transcript text, credentials, and raw audio are never logged. See [`AGENTS.md`](AGENTS.md) for diagnostic commands.

Licensed under [MIT](LICENSE). See [CONTRIBUTING.md](CONTRIBUTING.md) for project boundaries and validation steps.

## ASR benchmarks

Human-reviewed audio fixtures and their expected transcripts live in [`examples/benchmarks`](examples/benchmarks). The benchmark accepts M4A, MP3, WAV, and other formats decoded by `ffmpeg`, then compares the current streaming endpoint with optimized second-pass and non-streaming recognition using identical 200 ms PCM packets.

```bash
cargo run --manifest-path src-tauri/Cargo.toml --example asr_benchmark -- \
  examples/benchmarks/mandarin-basic-001
```

See the benchmark README for individual modes and hotword experiments. Credentials and decoded audio are never written to application logs.

## Validate

```bash
npm run build
cargo fmt --manifest-path src-tauri/Cargo.toml --check
cargo check --manifest-path src-tauri/Cargo.toml
```
