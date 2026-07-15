# Voice Flow

A lightweight, open-source Tauri + Rust app for system-wide, real-time voice input. It deliberately does not replace or bundle itself with an input method: keep the keyboard and typing habits you already use, hold a shortcut, watch the transcript appear, and release to insert at the active cursor.

The interface uses a quiet, warm, writing-focused visual system rather than a high-tech dashboard. The floating ribbon stays visible without taking focus from the app receiving text.

## MVP scope

- macOS runtime support today; Linux-facing boundaries are isolated for a later implementation.
- VolcEngine WebSocket V3 bidirectional streaming ASR.
- Legacy `APP ID + Access Token` authentication and current API-key-only authentication.
- Global hold-to-talk and toggle shortcuts.
- Manual microphone selection, with a system-default option.
- Live, non-focus-stealing transcript overlay.
- Optional VolcEngine semantic smoothing (DDC) for filler words and disfluencies, plus punctuation, inverse-text normalization, and a replaceable transcript-processing boundary.
- Automatic clipboard + paste insertion when dictation ends.
- Credentials stored locally in the Tauri app config directory, never in this repository.

## Run

Prerequisites: Rust, Node.js, npm, Xcode command-line tools, and a VolcEngine Speech account.

```bash
npm install
npm run tauri dev
```

On first use, macOS asks for microphone access. Automatic insertion also needs Accessibility permission for Voice Flow (or the terminal during development) under **System Settings → Privacy & Security → Accessibility**.

## Credentials

The settings screen supports both VolcEngine credential formats:

- Enter an **APP ID** and put the legacy **Access Token** in the Secret Key field; Voice Flow sends `X-Api-App-Key` and `X-Api-Access-Key`.
- Leave APP ID empty and put a current **API Key** in the Secret Key field; Voice Flow sends `X-Api-Key`.

The default ASR endpoint is `wss://openspeech.bytedance.com/api/v3/sauc/bigmodel`, using resource ID `volc.seedasr.sauc.duration`.

## Architecture

- `src-tauri/src/audio.rs`: microphone capture and 16 kHz mono PCM resampling.
- `src-tauri/src/asr/`: VolcEngine transport and binary protocol framing.
- `src-tauri/src/controller.rs`: dictation lifecycle and UI events.
- `src-tauri/src/platform/`: active-cursor insertion boundary; macOS is implemented, Linux is intentionally isolated.
- `src-tauri/src/text.rs`: final transcript-processing boundary for future semantic polish providers.
- `src-tauri/src/config.rs`: local settings and validation.
- `src/`: framework-free TypeScript UI for the settings window and dictation ribbon.

Provider policy (endpoint/resource ID), microphone, interaction mode, shortcut, and insertion behavior live in the settings model so future providers or Linux integrations do not require UI orchestration rewrites.

The MVP's “口语整理” uses VolcEngine DDC semantic smoothing together with ASR punctuation, number normalization, and deterministic whitespace cleanup. Model-based rewriting beyond spoken-language cleanup is intentionally listed in [`ROADMAP.md`](ROADMAP.md) instead of being presented as complete without a model provider and explicit privacy policy.

Licensed under [MIT](LICENSE). See [CONTRIBUTING.md](CONTRIBUTING.md) for project boundaries and validation steps.

## Validate

```bash
npm run build
cargo fmt --manifest-path src-tauri/Cargo.toml --check
cargo check --manifest-path src-tauri/Cargo.toml
```
