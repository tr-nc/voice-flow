# Baseline observations

Date: 2026-07-15

The expected transcript was supplied before recording and then read quickly by the speaker.

| Mode | Result | CER | Total time | Definite segment |
|---|---|---:|---:|---|
| `legacy` | severe substitution/deletion | 83.02% (44/53) | 6558 ms | yes |
| `current` | exact normalized match | 0.00% (0/53) | 6819 ms | yes |
| `nostream` | exact normalized match | 0.00% (0/53) | 6849 ms | yes |

The legacy first-pass endpoint returned `灰尘，灰尘，灰尘，中英文切换是否准确？`, losing nearly all of the mixed Chinese/English prefix. Current ASR second-pass recognition and the non-streaming model both recovered the complete sentence.

To check provider variance, `legacy` and `current` were each run three times. All three legacy runs produced the same 83.02% CER result, while all three current runs produced 0.00% CER. This case gives strong, reproducible evidence for enabling ASR second-pass recognition in production.
