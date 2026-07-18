/**
 * Presentation-only input for the dictation preview.
 *
 * Producers describe how text should feel; they do not expose recognition
 * engine concepts. A different engine only needs to adapt its output to this
 * small contract.
 */
export type PreviewTreatment = "floating" | "grounded";

export type PreviewChunk = {
  text: string;
  treatment: PreviewTreatment;
};

export type PreviewFrame = {
  chunks: readonly PreviewChunk[];
};

export type PreviewToken = {
  text: string;
  treatment: PreviewTreatment;
  whitespace: boolean;
};

const wordSegmenter = "Segmenter" in Intl ? new Intl.Segmenter(undefined, { granularity: "word" }) : undefined;

export function tokenizePreviewFrame(frame: PreviewFrame): PreviewToken[] {
  const chunks = frame.chunks.filter(({ text }) => text.length > 0);
  const text = chunks.map((chunk) => chunk.text).join("");
  const ranges: Array<{ start: number; end: number; treatment: PreviewTreatment }> = [];
  let rangeOffset = 0;
  for (const chunk of chunks) {
    ranges.push({ start: rangeOffset, end: rangeOffset + chunk.text.length, treatment: chunk.treatment });
    rangeOffset += chunk.text.length;
  }

  let tokenOffset = 0;
  return splitText(text).map((part) => {
    const tokenEnd = tokenOffset + part.length;
    // A word that straddles a treatment boundary keeps floating until the
    // whole word is grounded. Token boundaries therefore stay stable while a
    // producer advances its boundary through a word.
    const treatment = ranges.some(
      (range) => range.start < tokenEnd && range.end > tokenOffset && range.treatment === "floating",
    )
      ? "floating"
      : "grounded";
    tokenOffset = tokenEnd;
    return { text: part, treatment, whitespace: /^\s+$/u.test(part) };
  });
}

function splitText(text: string): string[] {
  if (!wordSegmenter) {
    // Older WebKitGTK builds may not expose Intl.Segmenter. Keep a dependency-
    // free fallback: CJK graphemes remain lively while Latin words stay whole.
    return text.match(/\r\n|\r|\n|\s+|[\p{Script=Han}\p{Script=Hiragana}\p{Script=Katakana}]|[\p{L}\p{N}\p{M}]+|[^\s]/gu) ?? [];
  }

  const parts: string[] = [];
  for (const { segment } of wordSegmenter.segment(text)) {
    // Keep line breaks separate so an animated inline token never owns layout
    // on both sides of a line boundary.
    parts.push(...segment.split(/(\r\n|\r|\n)/u).filter(Boolean));
  }
  return parts;
}

/**
 * Maps each next token to the previous token it can reuse. Matching ignores
 * treatment so a floating token can become grounded without losing its node.
 */
export function matchPreviewTokens(
  previous: readonly Pick<PreviewToken, "text">[],
  next: readonly Pick<PreviewToken, "text">[],
): Array<number | undefined> {
  const matches: Array<number | undefined> = new Array(next.length).fill(undefined);
  let prefix = 0;
  while (prefix < previous.length && prefix < next.length && previous[prefix].text === next[prefix].text) {
    matches[prefix] = prefix;
    prefix += 1;
  }

  let previousSuffix = previous.length - 1;
  let nextSuffix = next.length - 1;
  while (
    previousSuffix >= prefix &&
    nextSuffix >= prefix &&
    previous[previousSuffix].text === next[nextSuffix].text
  ) {
    matches[nextSuffix] = previousSuffix;
    previousSuffix -= 1;
    nextSuffix -= 1;
  }

  const previousMiddle = previous.slice(prefix, previousSuffix + 1);
  const nextMiddle = next.slice(prefix, nextSuffix + 1);
  if (previousMiddle.length === 0 || nextMiddle.length === 0) return matches;

  // Streaming text often grows inside its last word ("recog" →
  // "recognition"). Reuse that one node instead of presenting every added
  // character as a correction.
  if (
    previousMiddle.length === 1 &&
    nextMiddle.length === 1 &&
    nextMiddle[0].text.startsWith(previousMiddle[0].text)
  ) {
    matches[prefix] = prefix;
    return matches;
  }

  const middleMatches =
    previousMiddle.length * nextMiddle.length <= 40_000
      ? longestCommonSubsequence(previousMiddle, nextMiddle)
      : boundedGreedyMatches(previousMiddle, nextMiddle);

  for (const [nextIndex, previousIndex] of middleMatches) {
    matches[prefix + nextIndex] = prefix + previousIndex;
  }
  return matches;
}

function longestCommonSubsequence(
  previous: readonly Pick<PreviewToken, "text">[],
  next: readonly Pick<PreviewToken, "text">[],
): Array<[nextIndex: number, previousIndex: number]> {
  const columns = next.length + 1;
  const lengths = new Uint16Array((previous.length + 1) * columns);

  for (let previousIndex = 1; previousIndex <= previous.length; previousIndex += 1) {
    for (let nextIndex = 1; nextIndex <= next.length; nextIndex += 1) {
      const cell = previousIndex * columns + nextIndex;
      lengths[cell] =
        previous[previousIndex - 1].text === next[nextIndex - 1].text
          ? lengths[(previousIndex - 1) * columns + nextIndex - 1] + 1
          : Math.max(lengths[(previousIndex - 1) * columns + nextIndex], lengths[cell - 1]);
    }
  }

  const matches: Array<[number, number]> = [];
  let previousIndex = previous.length;
  let nextIndex = next.length;
  while (previousIndex > 0 && nextIndex > 0) {
    if (previous[previousIndex - 1].text === next[nextIndex - 1].text) {
      matches.push([nextIndex - 1, previousIndex - 1]);
      previousIndex -= 1;
      nextIndex -= 1;
    } else if (
      lengths[(previousIndex - 1) * columns + nextIndex] >=
      lengths[previousIndex * columns + nextIndex - 1]
    ) {
      previousIndex -= 1;
    } else {
      nextIndex -= 1;
    }
  }
  return matches.reverse();
}

function boundedGreedyMatches(
  previous: readonly Pick<PreviewToken, "text">[],
  next: readonly Pick<PreviewToken, "text">[],
): Array<[nextIndex: number, previousIndex: number]> {
  const matches: Array<[number, number]> = [];
  let previousIndex = 0;
  const lookAhead = 24;

  for (let nextIndex = 0; nextIndex < next.length && previousIndex < previous.length; nextIndex += 1) {
    const limit = Math.min(previous.length, previousIndex + lookAhead);
    let found = -1;
    for (let candidate = previousIndex; candidate < limit; candidate += 1) {
      if (previous[candidate].text === next[nextIndex].text) {
        found = candidate;
        break;
      }
    }
    if (found >= 0) {
      matches.push([nextIndex, found]);
      previousIndex = found + 1;
    }
  }
  return matches;
}
