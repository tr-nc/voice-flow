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
cargo check --manifest-path src-tauri/Cargo.toml
```

## Design constraints

- Keep the settings UI and live overlay lightweight and non-blocking.
- Never take focus from the application receiving dictation.
- Keep credentials and recordings local; never log secrets or raw microphone data.
- Put platform behavior behind `src-tauri/src/platform/`.
- Put speech-provider protocol behavior behind `src-tauri/src/asr/`.
- Keep transcript processing independent from capture and insertion.
- Prefer explicit settings over hard-coded product policy.

## Current platform status

macOS is the supported MVP target. Linux audio and shortcuts use cross-platform dependencies already, while cursor insertion remains an explicit platform task.
