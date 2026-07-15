# Baseline observations

Date: 2026-07-15

The expected transcript was supplied before recording and then read at normal speed by the speaker. The current result uses the production 400 ms VAD end window and real-time packet pacing.

| Mode | Accuracy | Live responsiveness | Stable follow | Total time |
|---|---:|---:|---:|---:|
| `current` | 100.00 | 100.00 | 100.00 | 12170 ms |
| `nostream` | 100.00 | n/a | 60.10 | 12691 ms |

Both modes produced 0.00% CER (0/53). For `current`, first-text lag was 266 ms, live update lag P95 was 425 ms, stable lag was 926 ms, and all stable text arrived before the final audio packet.

The scorer ignores case, spaces, hyphens, and punctuation, so `Voiceflow`/`VoiceFlow` and `real time`/`real-time` are equivalent to the expected text.
