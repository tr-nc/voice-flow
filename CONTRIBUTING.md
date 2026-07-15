# Contributing

Voice Flow is intentionally narrow: system-wide voice input without replacing the user's keyboard or input method.

## Development

```bash
npm install
npm run tauri dev
```

Before opening a change:

```bash
npm run build
cargo fmt --manifest-path src-tauri/Cargo.toml --check
cargo test --manifest-path src-tauri/Cargo.toml
cargo clippy --manifest-path src-tauri/Cargo.toml --all-targets -- -D warnings
```

## Design constraints

- Keep the settings UI and live overlay lightweight and non-blocking.
- Never take focus from the application receiving dictation.
- Keep credentials and recordings local; never log secrets, transcript text, or raw microphone data.
- Keep stdout and `voice-flow.log` tracing output equivalent.
- Put platform behavior behind `src-tauri/src/platform/`.
- Put speech-provider protocol behavior behind `src-tauri/src/asr/`.
- Keep side-specific shortcut handling behind `src-tauri/src/shortcut.rs`.
- Do not add model-based polish until its provider and privacy behavior are explicitly designed.
- Prefer explicit settings over hard-coded product policy.

## Current platform status

macOS and Linux are supported. Linux shortcut monitoring uses readable `evdev` keyboard devices and cursor insertion uses `uinput`; keep those implementations isolated from the macOS permission and insertion paths.
