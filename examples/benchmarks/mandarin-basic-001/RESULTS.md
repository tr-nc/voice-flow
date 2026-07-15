# Baseline observations

Date: 2026-07-15

The expected transcript was taken from the original, user-supplied recording name. It should remain human-reviewed ground truth rather than being replaced with provider output.

| Mode | Recognized difference | CER | Definite segment |
|---|---|---:|---|
| `current` | omitted `试` from `测试` | 5.88% (1/17) | yes |
| `optimized` | omitted `试` from `测试` | 5.88% (1/17) | yes |
| `nostream` | omitted `试` from `测试` | 5.88% (1/17) | yes |

`optimized` used `bigmodel_async` with ASR second-pass recognition. `nostream` was explicitly constrained to `zh-CN`. Both returned the same text as the current production endpoint for this case.

Temporary, uncommitted audio-processing variants were also tested through `optimized` or `nostream`:

- +8 dB gain
- band-pass filtering at 70–7600 Hz
- FFT denoising plus gain
- loudness normalization
- tempo factors 0.8, 0.9, and 1.1
- direct hotwords `测试` and `测试一下`

All retained the same one-character deletion. This single case therefore does not show an accuracy improvement from endpoint mode, simple DSP, or a direct hotword. More human-verified cases are required before changing production recognition policy based on CER.
