# ASR benchmarks

Each benchmark case lives in its own directory and contains:

- `benchmark.json`: case metadata and the human-authored expected transcript.
- An audio file referenced by `audio`. M4A, MP3, WAV, and other formats supported by the local `ffmpeg` installation are accepted.

Run every provider mode against a case from the repository root:

```bash
cargo run --manifest-path src-tauri/Cargo.toml --example asr_benchmark -- \
  examples/benchmarks/mandarin-basic-001
```

Run one mode, optionally with repeatable ASR hotwords:

```bash
cargo run --manifest-path src-tauri/Cargo.toml --example asr_benchmark -- \
  examples/benchmarks/mandarin-basic-001 --mode optimized \
  --hotword 'Voice Flow'
```

Modes:

- `current`: current production endpoint (`bigmodel`).
- `optimized`: optimized bidirectional endpoint (`bigmodel_async`) with ASR second-pass recognition enabled.
- `nostream`: higher-accuracy streaming-input endpoint (`bigmodel_nostream`).

The tool decodes every source to the same 16 kHz mono signed 16-bit PCM stream, sends 200 ms packets in real time, and reports punctuation-insensitive character error rate (CER). The optional `--hotword` argument is intended for explicit experiments and is not part of the default baseline. It reads the Secret Key from `VOICE_FLOW_SECRET_KEY` or the local Voice Flow settings file. It never writes credentials, decoded PCM, provider transcripts, or benchmark results to the application log.

Add future cases by copying a case directory, choosing a stable case ID, retaining the original M4A/MP3/WAV source, and manually verifying `expected_text` against the recording. Do not derive ground truth from an ASR result.
