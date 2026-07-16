# ASR benchmarks

Each benchmark case lives in its own directory and contains:

- `benchmark.json`: case metadata and the human-authored expected transcript.
- An audio file referenced by `audio`. M4A, MP3, WAV, and other formats supported by the local `ffmpeg` installation are accepted.

Run every provider mode against a case from the repository root:

```bash
cargo run --manifest-path src-tauri/Cargo.toml --example asr_benchmark -- \
  examples/benchmarks/code-switch-001-normal
```

Run one mode, optionally with repeatable ASR hotwords or experimental VAD settings:

```bash
cargo run --manifest-path src-tauri/Cargo.toml --example asr_benchmark -- \
  examples/benchmarks/code-switch-long-001 --mode current \
  --hotword 'Voice Flow' --end-window-ms 400
```

`--force-to-speech-ms` is also available for controlled experiments and requires `--end-window-ms`.

Modes:

- `current`: production optimized bidirectional endpoint (`bigmodel_async`) with ASR second-pass recognition enabled.
- `nostream`: higher-accuracy streaming-input endpoint (`bigmodel_nostream`).

The tool decodes every source to the same 16 kHz mono signed 16-bit PCM stream, removes only leading and trailing silence, then adds the same 200 ms edge guards used by production. Trimming the fixture first stress-tests speech that begins and ends directly at the push-to-talk boundary; the explicit guards keep provider framing consistent without relying on accidental silence in a recording. Each 200 ms packet is sent when that audio would become available in real time. The tool reports the following independent scores and the raw measurements behind them:

- **Accuracy**: `max(0, 100 - CER)`. CER ignores case, spaces, hyphens, and punctuation. Substitutions, insertions, and deletions are reported separately.
- **Live responsiveness**: the mean of first-text lag and P95 provisional-update lag. A lag of at most 500 ms scores 100; 2500 ms or more scores zero, with linear interpolation between them. This is not applicable to `nostream`.
- **Stable follow**: 50% P95 `definite` result lag, 30% of stable text returned before the final audio packet, and 20% final-tail latency. Stable lag scores 100 at 1200 ms or less and zero at 4000 ms or more. Final-tail latency scores 100 at 500 ms or less and zero at 3000 ms or more.

For two-pass recognition, stable lag starts at the estimated end of speech, excluding the configured VAD silence window. Scores are diagnostic rather than a substitute for their raw P50/P95 latency and coverage values.

The optional tuning arguments are intended for explicit experiments and are not part of a fixture's ground truth. The tool reads the Secret Key from `VOICE_FLOW_SECRET_KEY` or the local Voice Flow settings file. It never writes credentials, decoded PCM, provider transcripts, or benchmark results to the application log.

Add future cases by copying a case directory, choosing a stable case ID, retaining the original M4A/MP3/WAV source, and manually verifying `expected_text` against the recording. Do not derive ground truth from an ASR result.
