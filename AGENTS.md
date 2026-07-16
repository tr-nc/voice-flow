# AGENTS.md

## Project

- App: Voice Flow
- Stack: Rust + Tauri 2 + framework-free TypeScript
- Current platform: macOS
- Future platform: Linux; keep platform-specific behavior behind `src-tauri/src/platform/` or another explicit platform boundary.
- Product scope: lightweight real-time streaming ASR and cursor insertion. Do not add LLM polish until a model provider, configuration, and privacy behavior are explicitly designed.

## Runtime logs

Voice Flow writes one fixed log file per platform:

```text
# macOS
~/Library/Logs/dev.voiceflow.desktop/voice-flow.log

# Linux
~/.local/share/dev.voiceflow.desktop/logs/voice-flow.log
```

The same tracing output is written to stdout and the file. The file appends across launches and is truncated at startup when it reaches 5 MiB; no rotated log files are created.

Useful diagnostics on Linux (substitute the macOS path when applicable):

```bash
tail -n 300 "$HOME/.local/share/dev.voiceflow.desktop/logs/voice-flow.log"
rg -n "ERROR|WARN|ASR|microphone|shortcut" "$HOME/.local/share/dev.voiceflow.desktop/logs/voice-flow.log"
```

Never write credentials, transcript text, or raw audio to the global log. Logging transcript lengths, packet sizes, timings, state transitions, device names, session IDs, and non-secret provider metadata is allowed. Human-reviewed expected transcripts and user-approved audio under `examples/benchmarks/` are test fixtures rather than runtime logs; benchmark tools may print recognition results to their invoking terminal but must not write them to the application log.

## Session diagnostics

Voice Flow retains the latest 100 dictations as self-contained local folders:

```text
# macOS
~/Library/Application Support/dev.voiceflow.desktop/diagnostics/sessions/<timestamp>_<session-id>/

# Linux
~/.local/share/dev.voiceflow.desktop/diagnostics/sessions/<timestamp>_<session-id>/
```

Each folder contains `session.json` with partial/final transcripts, timings, audio metadata, outcome, and error details, plus `audio.wav` with the captured 16 kHz mono PCM before synthetic ASR edge guards. These files intentionally contain sensitive user content, use user-only permissions, must never be committed or uploaded automatically, and are correlated to the global log by `session_id`.

When investigating a user report without an exact time, search all retained `session.json` files across `final_text` and `transcript_updates`, rank approximate matches by local start time, then inspect the matching WAV and global-log lines. The session folders are the source of truth; do not add a database or persistent search index for this 100-record store.

Local settings, including credentials, are stored at:

```text
~/Library/Application Support/dev.voiceflow.desktop/settings.json
```

Never commit that file or print its secret values.

## Validation

Run from the repository root:

```bash
npm run build
cargo fmt --manifest-path src-tauri/Cargo.toml --check
cargo test --manifest-path src-tauri/Cargo.toml
cargo test --manifest-path src-tauri/Cargo.toml --example asr_benchmark
cargo clippy --manifest-path src-tauri/Cargo.toml --all-targets -- -D warnings
npx tauri build --debug --no-bundle
```
