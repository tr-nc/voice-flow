# Voice Flow

A lightweight, open-source Tauri + Rust app for system-wide, real-time voice input. It deliberately does not replace or bundle itself with an input method: keep the keyboard and typing habits you already use, hold a shortcut, watch the transcript appear, and release to insert at the active cursor.

The interface uses a quiet, warm, writing-focused visual system rather than a high-tech dashboard. The floating overlay contains only recognized text, accepts no input, and never takes focus from the app receiving text.

## MVP scope

- macOS and Linux (Wayland or X11) runtime support.
- VolcEngine WebSocket V3 bidirectional streaming ASR.
- One locally stored Secret Key for VolcEngine authentication.
- Global hold-to-talk and toggle shortcuts using any supported single key or key chord, including left/right modifier distinction.
- Manual microphone selection, with a system-default option.
- Live, click-through transcript-only overlay with no controls or decorative chrome.
- Raw streaming ASR output with VolcEngine punctuation and inverse-text normalization; no LLM polish in the MVP.
- Automatic clipboard + paste insertion when dictation ends.
- Credentials stored locally in the Tauri app config directory, never in this repository.

## Run

Prerequisites: Rust, Node.js, npm, and a VolcEngine Speech account.

### macOS

Install the Xcode command-line tools, then run:

```bash
npm install
npm run tauri dev
```

On first use, macOS asks for microphone access. Global side-specific shortcut detection and automatic insertion also need Accessibility permission for Voice Flow (or the terminal during development) under **System Settings → Privacy & Security → Accessibility**.

### Linux

Install the Tauri/WebKit and audio build dependencies. On Fedora:

```bash
sudo dnf install gcc gtk3-devel webkit2gtk4.1-devel alsa-lib-devel wl-clipboard
```

Voice Flow reads Linux input event devices so hold shortcuts and left/right modifiers work under both Wayland and X11. Grant that access once, load the virtual-input module used for automatic paste, then **sign out and back in** so the new group membership takes effect:

```bash
sudo usermod -aG input "$USER"
sudo modprobe uinput
```

Membership in `input` permits applications running as your user to observe keyboard events; only grant it on a machine you control. `/dev/uinput` must also be writable by the logged-in user (Fedora grants this through an active-session ACL). Voice Flow never logs pressed keys.

After signing back in:

```bash
npm install
npm run tauri dev
```

The Linux default shortcut is right Control. Automatic insertion uses `wl-copy` plus a virtual `Ctrl+Shift+V` on Wayland, and the X11 clipboard plus the same virtual shortcut on X11. Voice Flow uses this one paste chord for every Linux application without application-specific handling. Enter the Secret Key again on a new computer; local settings are intentionally not synchronized.

## Local install

Voice Flow does not need a release package for personal use. From either macOS or Fedora, build and install the current checkout with:

```bash
npm run install:local
```

The command runs a release build and installs only for the current user:

- macOS: `~/Applications/Voice Flow.app`
- Fedora: `~/.local/bin/voice-flow`, plus an application-menu entry

Run the same command after pulling or making changes to replace the installed build. Settings and credentials remain in the platform config directory and are not touched. If Voice Flow is already running, restart it after installation.

The installer deliberately does not use `sudo` or change operating-system policy. Complete the Linux dependency, `input` group, and `uinput` setup above once. To load `uinput` automatically after future Fedora reboots, run:

```bash
printf 'uinput\n' | sudo tee /etc/modules-load.d/voice-flow.conf
sudo modprobe uinput
```

On macOS, launch the installed application once and grant **Microphone** and **Accessibility** permission to Voice Flow. Development runs launched through a terminal may have separate macOS permission records.

## Credentials and settings

Enter the VolcEngine **Secret Key** once. It is stored only in the local settings file. The Secret Key, selected microphone, shortcut, interaction mode, and insertion preference are saved automatically whenever they change.

Voice Flow uses the optimized bidirectional ASR endpoint `wss://openspeech.bytedance.com/api/v3/sauc/bigmodel_async`, enables ASR second-pass recognition with a benchmarked 400 ms VAD end window for accurate, responsive stable text, and uses resource ID `volc.seedasr.sauc.duration`.

## Architecture

- `src-tauri/src/audio.rs`: microphone capture and 16 kHz mono PCM resampling.
- `src-tauri/src/asr/`: VolcEngine transport and binary protocol framing.
- `src-tauri/src/controller.rs`: dictation lifecycle and UI events.
- `src-tauri/src/diagnostics.rs`: self-contained recent-session audio and recognition metadata.
- `src-tauri/src/shortcut.rs`: arbitrary key/chord polling with left/right modifier distinction.
- `src-tauri/src/logging.rs`: identical stdout and fixed-file tracing output.
- `src-tauri/src/platform/`: active-cursor insertion boundary; macOS uses System Events and Linux uses `uinput`.
- `src-tauri/src/config.rs`: local settings and validation.
- `src/`: framework-free TypeScript UI for the settings window and transcript-only overlay.

Provider endpoint/resource policy is owned by the backend. User choices such as microphone, interaction mode, shortcut, and insertion behavior live in the automatically persisted settings model.

Runtime logs are written to stdout and one platform-local file with identical content:

- macOS: `~/Library/Logs/dev.voiceflow.desktop/voice-flow.log`
- Linux: `~/.local/share/dev.voiceflow.desktop/logs/voice-flow.log`

The global log does not contain transcript text, credentials, pressed keys, or raw audio. Every dictation has a shared `session_id` in the global log and its local diagnostic record.

For reproducible troubleshooting, Voice Flow retains the latest 100 dictation sessions as self-contained folders containing `audio.wav` (16 kHz mono PCM captured before synthetic ASR edge guards) and `session.json` (timestamps, partial/final transcripts, audio format, outcome, and errors):

- macOS: `~/Library/Application Support/dev.voiceflow.desktop/diagnostics/sessions/`
- Linux: `~/.local/share/dev.voiceflow.desktop/diagnostics/sessions/`

The oldest folder is deleted when the limit is exceeded. These records can contain sensitive speech and transcript content, remain local with user-only filesystem permissions, and are never uploaded automatically. See [`AGENTS.md`](AGENTS.md) for diagnostic commands.

Licensed under [MIT](LICENSE). See [CONTRIBUTING.md](CONTRIBUTING.md) for project boundaries and validation steps.

## ASR benchmarks

Human-reviewed audio fixtures and their expected transcripts live in [`examples/benchmarks`](examples/benchmarks). The benchmark accepts M4A, MP3, WAV, and other formats decoded by `ffmpeg`, trims boundary silence to model immediate push-to-talk speech, adds the same explicit edge guards as production, then compares current second-pass and non-streaming recognition using identical real-time-paced 200 ms PCM packets. It independently scores final accuracy, live first-pass responsiveness, and stable second-pass follow latency while retaining the raw measurements.

```bash
cargo run --manifest-path src-tauri/Cargo.toml --example asr_benchmark -- \
  examples/benchmarks/code-switch-long-001
```

See the benchmark README for scoring formulas, individual modes, VAD tuning, and hotword experiments. Credentials and decoded audio are never written to application logs.

## Validate

```bash
npm run build
cargo fmt --manifest-path src-tauri/Cargo.toml --check
cargo check --manifest-path src-tauri/Cargo.toml
```
