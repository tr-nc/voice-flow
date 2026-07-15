# Baseline observations

Date: 2026-07-15

The expected transcript was supplied before recording and then read at normal speed with natural pauses. The source is 35.776 seconds long. Scores use the formulas documented in the benchmark README.

## Production baseline

Production `current` uses a 400 ms VAD end window.

| Metric | `current` | `nostream` |
|---|---:|---:|
| Accuracy score | 100.00 | 100.00 |
| CER | 0.00% (0/210) | 0.00% (0/210) |
| Live responsiveness score | 100.00 | n/a |
| First-text lag | 178 ms | n/a |
| Live update lag P50 / P95 | 180 / 228 ms | n/a |
| Stable follow score | 94.05 | 43.43 |
| Stable lag P50 / P95 | 1069 / 1533 ms | 3417 / 9596 ms |
| Stable text before final audio | 100.00% | 78.10% |
| Final-tail latency | 72 ms | 352 ms |

`current` returned three second-pass `definite` segments before the recording ended. Together they contained all 210 normalized characters. The last stable segment arrived at 33.572 seconds, about 2.2 seconds before the final audio packet. This confirms that second-pass recognition follows completed speech segments during an open stream rather than waiting for key release.

The raw outputs differed from the expected text only in punctuation, capitalization, spacing, and hyphenation, which the CER normalization intentionally ignores.

## Repeatability

Across five 400 ms `current` runs:

- Accuracy remained 100.00% in every run.
- Live responsiveness remained 100.00% in every run.
- Stable follow ranged from 94.05 to 97.29.
- Stable P95 lag ranged from 1352 to 1533 ms.
- Every run stabilized 100% of recognized text before the final audio packet.

## VAD tuning

| End window | Long-case accuracy | Long stable P95 | Other-case result | Decision |
|---:|---:|---:|---|---|
| 800 ms | 100.00 | roughly 1.60–1.74 s | accurate | slower baseline |
| 600 ms | 100.00 | 1.68 s in the sampled run | accurate | no consistent advantage |
| 400 ms | 100.00 | 1.35–1.53 s | normal and fast cases remained exact | production default |
| 200 ms | 100.00 | below 1.0 s | normal case fell to 88.68% | rejected |

The 200 ms setting split `speech recognition` across very short segments and produced `speech. To recognize`, demonstrating that minimizing VAD silence alone can improve latency scores while materially harming accuracy. The 400 ms setting is the best tested balance across all retained fixtures.
