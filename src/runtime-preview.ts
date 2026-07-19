import type { PreviewFrame } from "./preview-model";

export type PreviewRuntimePhase =
  | "idle"
  | "connecting"
  | "listening"
  | "finalizing"
  | "inserting"
  | "complete"
  | "error";

export const READY_PROMPT = "Your mic is ready start speaking";

export function toRuntimePreviewFrame(
  phase: PreviewRuntimePhase,
  transcriptFrame: PreviewFrame,
): PreviewFrame {
  const hasTranscript = transcriptFrame.chunks.some(({ text }) => text.length > 0);
  if (!hasTranscript && (phase === "connecting" || phase === "listening")) {
    return {
      chunks: [{ text: READY_PROMPT, treatment: "settled" }],
      prompt: true,
    };
  }

  return {
    ...transcriptFrame,
    final: hasTranscript && (phase === "inserting" || phase === "complete"),
  };
}
