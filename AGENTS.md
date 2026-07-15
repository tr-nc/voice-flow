# AGENTS.md

## Project

- App: Voice Flow
- Stack: Rust + Tauri 2 + framework-free TypeScript
- Current platform: macOS
- Future platform: Linux; keep platform-specific behavior behind `src-tauri/src/platform/` or another explicit platform boundary.
- Product scope: lightweight real-time streaming ASR and cursor insertion. Do not add LLM polish until a model provider, configuration, and privacy behavior are explicitly designed.

## Runtime logs

Voice Flow writes one fixed log file on macOS:

```text
~/Library/Logs/dev.voiceflow.desktop/voice-flow.log
```

The same tracing output is written to stdout and the file. The file appends across launches and is truncated at startup when it reaches 5 MiB; no rotated log files are created.

Useful diagnostics:

```bash
tail -n 300 "$HOME/Library/Logs/dev.voiceflow.desktop/voice-flow.log"
rg -n "ERROR|WARN|ASR|microphone|shortcut" "$HOME/Library/Logs/dev.voiceflow.desktop/voice-flow.log"
```

Never log credentials, transcript text, or raw audio. Logging transcript lengths, packet sizes, timings, state transitions, device names, and non-secret provider metadata is allowed.

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
cargo clippy --manifest-path src-tauri/Cargo.toml --all-targets -- -D warnings
npx tauri build --debug --no-bundle
```
