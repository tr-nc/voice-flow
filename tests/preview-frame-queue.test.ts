import assert from "node:assert/strict";
import test from "node:test";
import { PreviewFrameQueue } from "../src/preview-frame-queue.ts";
import type { PreviewFrame } from "../src/preview-model.ts";

const textFrame = (text: string): PreviewFrame => ({
  chunks: [{ text, treatment: "processing" }],
});

test("coalesces streaming updates into the latest animation frame", () => {
  const callbacks = new Map<number, () => void>();
  const rendered: PreviewFrame[] = [];
  let nextFrame = 1;
  const queue = new PreviewFrameQueue((frame) => rendered.push(frame), {
    request(callback) {
      const frame = nextFrame++;
      callbacks.set(frame, callback);
      return frame;
    },
    cancel(frame) {
      callbacks.delete(frame);
    },
  });

  queue.submit(textFrame("first"));
  queue.submit(textFrame("latest"));

  assert.equal(callbacks.size, 1);
  callbacks.get(1)?.();
  assert.deepEqual(rendered, [textFrame("latest")]);
});

test("an empty session boundary releases a frame suspended while hidden", () => {
  const callbacks = new Map<number, () => void>();
  const rendered: PreviewFrame[] = [];
  const cancelled: number[] = [];
  let nextFrame = 1;
  const queue = new PreviewFrameQueue((frame) => rendered.push(frame), {
    request(callback) {
      const frame = nextFrame++;
      callbacks.set(frame, callback);
      return frame;
    },
    cancel(frame) {
      cancelled.push(frame);
      callbacks.delete(frame);
    },
  });

  queue.submit(textFrame("previous session"));
  queue.submit({ chunks: [] });

  assert.deepEqual(cancelled, [1]);
  assert.deepEqual(rendered, [{ chunks: [] }]);

  queue.submit(textFrame("next session"));
  assert.equal(callbacks.size, 1);
  callbacks.get(2)?.();
  assert.deepEqual(rendered, [{ chunks: [] }, textFrame("next session")]);
});

test("the final insertion frame replaces a pending partial immediately", () => {
  const callbacks = new Map<number, () => void>();
  const rendered: PreviewFrame[] = [];
  const cancelled: number[] = [];
  let nextFrame = 1;
  const queue = new PreviewFrameQueue((frame) => rendered.push(frame), {
    request(callback) {
      const frame = nextFrame++;
      callbacks.set(frame, callback);
      return frame;
    },
    cancel(frame) {
      cancelled.push(frame);
      callbacks.delete(frame);
    },
  });
  const finalFrame: PreviewFrame = {
    chunks: [{ text: "polished final", treatment: "settled" }],
    final: true,
  };

  queue.submit(textFrame("streaming partial"));
  queue.submit(finalFrame);

  assert.deepEqual(cancelled, [1]);
  assert.equal(callbacks.size, 0);
  assert.deepEqual(rendered, [finalFrame]);
});
