import assert from "node:assert/strict";
import test from "node:test";
import {
  matchPreviewTokens,
  samePreviewTokens,
  tokenizePreviewFrame,
  type PreviewToken,
} from "../src/preview-model.ts";

test("tokenization preserves text and presentation treatment", () => {
  const tokens = tokenizePreviewFrame({
    chunks: [
      { text: "已经落地。", treatment: "grounded" },
      { text: " still floating", treatment: "floating" },
    ],
  });

  assert.equal(tokens.map(({ text }) => text).join(""), "已经落地。 still floating");
  assert.ok(tokens.filter(({ treatment }) => treatment === "grounded").length > 0);
  assert.ok(tokens.filter(({ treatment }) => treatment === "floating").length > 0);
  assert.equal(tokens.find(({ text }) => /^\s+$/u.test(text))?.whitespace, true);
});

test("unchanged tokens keep identity when treatment changes", () => {
  const previous: PreviewToken[] = [
    { text: "同一个词", treatment: "floating", whitespace: false },
    { text: "。", treatment: "floating", whitespace: false },
  ];
  const next: PreviewToken[] = previous.map((token) => ({ ...token, treatment: "grounded" }));

  assert.deepEqual(matchPreviewTokens(previous, next), [0, 1]);
});

test("only a truly unchanged visual frame can skip rendering", () => {
  const floating: PreviewToken[] = [{ text: "相同文字", treatment: "floating", whitespace: false }];

  assert.equal(samePreviewTokens(floating, floating.map((token) => ({ ...token }))), true);
  assert.equal(
    samePreviewTokens(floating, [{ text: "相同文字", treatment: "grounded", whitespace: false }]),
    false,
  );
});

test("a treatment boundary inside a word does not split its visual token", () => {
  const transitioning = tokenizePreviewFrame({
    chunks: [
      { text: "recog", treatment: "grounded" },
      { text: "nition", treatment: "floating" },
    ],
  });
  const grounded = tokenizePreviewFrame({ chunks: [{ text: "recognition", treatment: "grounded" }] });

  assert.deepEqual(transitioning.map(({ text }) => text), grounded.map(({ text }) => text));
  assert.equal(transitioning[0].treatment, "floating");
  assert.equal(grounded[0].treatment, "grounded");
});

test("revision reuses surrounding tokens and replaces only the changed middle", () => {
  const token = (text: string) => ({ text });
  const previous = ["今天", "天气", "很好", "。"].map(token);
  const next = ["今天", "天气", "非常", "好", "。"].map(token);

  assert.deepEqual(matchPreviewTokens(previous, next), [0, 1, undefined, undefined, 3]);
});

test("insertions do not disturb stable suffix identity", () => {
  const token = (text: string) => ({ text });
  const previous = ["A", "B", "D"].map(token);
  const next = ["A", "B", "C", "D"].map(token);

  assert.deepEqual(matchPreviewTokens(previous, next), [0, 1, undefined, 2]);
});

test("a growing streaming word reuses its node without a correction ghost", () => {
  const token = (text: string) => ({ text });

  assert.deepEqual(matchPreviewTokens([token("recog")], [token("recognition")]), [0]);
  assert.deepEqual(
    matchPreviewTokens([token("say"), token(" "), token("recog"), token(".")], [
      token("say"),
      token(" "),
      token("recognition"),
      token("."),
    ]),
    [0, 1, 2, 3],
  );
});
