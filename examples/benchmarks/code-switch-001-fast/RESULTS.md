# Baseline observations

Date: 2026-07-15

The expected transcript was supplied before recording and then read quickly by the speaker.

| Mode | Result | CER | Total time | Definite segment |
|---|---|---:|---:|---|
| `current` | severe substitution/deletion | 83.02% (44/53) | 6558 ms | yes |
| `optimized` | exact normalized match | 0.00% (0/53) | 6819 ms | yes |
| `nostream` | exact normalized match | 0.00% (0/53) | 6849 ms | yes |

The current first-pass endpoint returned `灰尘，灰尘，灰尘，中英文切换是否准确？`, losing nearly all of the mixed Chinese/English prefix. The optimized ASR second pass and the non-streaming model both recovered the complete sentence.

To check provider variance, `current` and `optimized` were each run three times. All three current runs produced the same 83.02% CER result, while all three optimized runs produced 0.00% CER. This case gives strong, reproducible evidence for enabling ASR second-pass recognition in production.
