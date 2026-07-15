# Baseline observations

Date: 2026-07-15

The expected transcript was supplied before recording and then read quickly by the speaker. The current result uses the production 400 ms VAD end window and real-time packet pacing.

| Mode | Accuracy | Live responsiveness | Stable follow | Total time |
|---|---:|---:|---:|---:|
| `current` | 100.00 | 100.00 | 68.92 | 7298 ms |
| `nostream` | 100.00 | n/a | 70.00 | 7130 ms |

Both modes produced 0.00% CER (0/53). For `current`, first-text lag was 154 ms and live update lag P95 was 190 ms. The recording ended before its only stable segment arrived, so pre-final stable coverage was 0%; the final-tail latency was 635 ms.

Current ASR second-pass recognition remained exact despite the fast delivery. The lower stable-follow score reflects the lack of trailing time after speech rather than a text-accuracy failure.
