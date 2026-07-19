import assert from "node:assert/strict";
import test from "node:test";
import { READY_PROMPT, toRuntimePreviewFrame } from "../src/runtime-preview.ts";

test("shows an English microphone prompt before speech begins", () => {
  assert.equal(READY_PROMPT, "Your mic is ready start speaking");
  assert.doesNotMatch(READY_PROMPT, /[^\p{L}\p{N}\s]/u);
  assert.deepEqual(toRuntimePreviewFrame("connecting", { chunks: [] }), {
    chunks: [{ text: READY_PROMPT, treatment: "settled" }],
    prompt: true,
  });
  assert.equal(toRuntimePreviewFrame("listening", { chunks: [] }).prompt, true);
  assert.deepEqual(toRuntimePreviewFrame("idle", { chunks: [] }), {
    chunks: [],
    final: false,
  });
});

test("the final preview uses the exact canonical text sent for insertion", () => {
  const transcript = "The polished final transcript.";
  const frame = toRuntimePreviewFrame("inserting", {
    chunks: [{ text: transcript, treatment: "settled" }],
  });

  assert.equal(frame.chunks.map(({ text }) => text).join(""), transcript);
  assert.equal(frame.final, true);
  assert.equal(frame.prompt, undefined);
});
