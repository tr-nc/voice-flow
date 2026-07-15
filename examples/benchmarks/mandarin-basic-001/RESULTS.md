# Baseline observations

Date: 2026-07-15

The expected transcript was manually confirmed by the speaker. Ground truth must remain human-reviewed rather than being derived from provider output or the source filename.

| Mode | Result | CER | Definite segment |
|---|---|---:|---|
| `current` | exact match | 0.00% (0/16) | yes |
| `optimized` | exact match | 0.00% (0/16) | yes |
| `nostream` | exact match | 0.00% (0/16) | yes |

`optimized` used `bigmodel_async` with ASR second-pass recognition. `nostream` was explicitly constrained to `zh-CN`. All three modes produced the human-confirmed transcript exactly, so this case does not distinguish their recognition accuracy. More human-verified cases, especially known failure cases, are required before changing production recognition policy based on CER.
