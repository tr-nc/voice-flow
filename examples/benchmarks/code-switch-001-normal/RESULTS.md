# Baseline observations

Date: 2026-07-15

The expected transcript was supplied before recording and then read at normal speed by the speaker.

| Mode | Result | CER | Total time | Definite segment |
|---|---|---:|---:|---|
| `current` | inserted `s` after `real time` | 1.89% (1/53) | 12081 ms | yes |
| `optimized` | exact normalized match | 0.00% (0/53) | 12111 ms | yes |
| `nostream` | exact normalized match | 0.00% (0/53) | 12428 ms | yes |

The scorer ignores case, spaces, hyphens, and punctuation, so `Voiceflow`/`VoiceFlow` and `real time`/`real-time` are equivalent to the expected text. The current first-pass endpoint produced `real times`; ASR second-pass recognition removed the extra `s` with effectively no additional end-to-end latency in this run.
