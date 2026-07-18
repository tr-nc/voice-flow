import assert from "node:assert/strict";
import test from "node:test";
import { latestWholeLineScrollTop } from "../src/preview-layout.ts";

test("does not scroll a preview that fits", () => {
  assert.equal(
    latestWholeLineScrollTop({
      scrollHeight: 254,
      clientHeight: 262,
      paddingTop: 12,
      lineHeight: 25.6,
    }),
    0,
  );
});

test("keeps the first visible row on a complete line boundary", () => {
  assert.equal(
    latestWholeLineScrollTop({
      scrollHeight: 280,
      clientHeight: 262,
      paddingTop: 12,
      lineHeight: 25.6,
    }),
    12,
  );
  assert.equal(
    latestWholeLineScrollTop({
      scrollHeight: 306,
      clientHeight: 262,
      paddingTop: 12,
      lineHeight: 25.6,
    }),
    37.6,
  );
});

test("uses the regular bottom position when line metrics are unavailable", () => {
  assert.equal(
    latestWholeLineScrollTop({
      scrollHeight: 306,
      clientHeight: 262,
      paddingTop: 12,
      lineHeight: Number.NaN,
    }),
    44,
  );
});
