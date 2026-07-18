import type { PreviewFrame } from "./preview-model";

export type StabilitySegment = {
  text: string;
  definite: boolean;
};

/**
 * The only recognition-to-presentation adapter. Canonical text remains the
 * source of truth; segment text is used solely to locate its settled prefix.
 */
export function toPreviewFrame(text: string, segments: readonly StabilitySegment[]): PreviewFrame {
  if (!text) return { chunks: [] };

  const settledOffset = findSettledOffset(text, segments);
  if (settledOffset === 0) {
    return { chunks: [{ text, treatment: "processing" }] };
  }

  const settled = text.slice(0, settledOffset);
  const processing = text.slice(settledOffset);
  return {
    chunks: [
      ...(settled ? [{ text: settled, treatment: "settled" as const }] : []),
      ...(processing ? [{ text: processing, treatment: "processing" as const }] : []),
    ],
  };
}

/**
 * Maps the leading continuous definite segments into canonical UTF-16 text.
 * Formatting-only differences are tolerated, while any spoken-content
 * mismatch fails closed so the renderer never marks uncertain words settled.
 */
export function findSettledOffset(text: string, segments: readonly StabilitySegment[]): number {
  let definitePrefix = "";
  for (const segment of segments) {
    if (!segment.definite) break;
    definitePrefix += segment.text;
  }

  const definiteUnits = comparableUnits(definitePrefix);
  if (definiteUnits.length === 0) return 0;

  const canonicalUnits = comparableUnits(text);
  if (canonicalUnits.length < definiteUnits.length) return 0;
  for (let index = 0; index < definiteUnits.length; index += 1) {
    if (canonicalUnits[index].value !== definiteUnits[index].value) return 0;
  }

  let offset = canonicalUnits[definiteUnits.length - 1].end;
  for (const character of text.slice(offset)) {
    if (!isFormatting(character)) break;
    offset += character.length;
  }
  return offset;
}

type ComparableUnit = {
  value: string;
  end: number;
};

function comparableUnits(text: string): ComparableUnit[] {
  const units: ComparableUnit[] = [];
  let offset = 0;
  for (const character of text) {
    const end = offset + character.length;
    if (!isFormatting(character)) {
      for (const normalized of character.normalize("NFKC").toLocaleLowerCase()) {
        units.push({ value: normalized, end });
      }
    }
    offset = end;
  }
  return units;
}

function isFormatting(character: string): boolean {
  return /[\s\p{P}\p{Cf}]/u.test(character);
}
