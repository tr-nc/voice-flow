import assert from "node:assert/strict";
import test from "node:test";
import { findSettledOffset, toPreviewFrame } from "../src/preview-adapter.ts";

test("maps an exact definite prefix into settled and processing chunks", () => {
  assert.deepEqual(
    toPreviewFrame("第一句。第二句", [
      { text: "第一句。", definite: true },
      { text: "第二句", definite: false },
    ]),
    {
      chunks: [
        { text: "第一句。", treatment: "settled" },
        { text: "第二句", treatment: "processing" },
      ],
    },
  );
});

test("tolerates canonical punctuation whitespace width and case differences", () => {
  assert.deepEqual(
    toPreviewFrame("Ｈello，世界！ 下一句", [
      { text: "hello 世界", definite: true },
      { text: "下一句", definite: false },
    ]),
    {
      chunks: [
        { text: "Ｈello，世界！ ", treatment: "settled" },
        { text: "下一句", treatment: "processing" },
      ],
    },
  );
});

test("same canonical text can transition from processing to settled", () => {
  const text = "文字没有改变。";
  assert.deepEqual(toPreviewFrame(text, [{ text, definite: false }]), {
    chunks: [{ text, treatment: "processing" }],
  });
  assert.deepEqual(toPreviewFrame(text, [{ text, definite: true }]), {
    chunks: [{ text, treatment: "settled" }],
  });
});

test("spoken-content mismatches fail closed", () => {
  const text = "正确的原文";
  assert.equal(findSettledOffset(text, [{ text: "不同的内容", definite: true }]), 0);
  assert.deepEqual(toPreviewFrame(text, [{ text: "不同的内容", definite: true }]), {
    chunks: [{ text, treatment: "processing" }],
  });
});

test("only a leading continuous definite run can settle", () => {
  assert.equal(
    findSettledOffset("第一句第二句", [
      { text: "第一句", definite: false },
      { text: "第二句", definite: true },
    ]),
    0,
  );
});
