export type PreviewScrollMetrics = {
  scrollHeight: number;
  clientHeight: number;
  paddingTop: number;
  lineHeight: number;
};

/** Keep the newest text visible without clipping the first visible line. */
export function latestWholeLineScrollTop({
  scrollHeight,
  clientHeight,
  paddingTop,
  lineHeight,
}: PreviewScrollMetrics): number {
  const maxScrollTop = Math.max(0, scrollHeight - clientHeight);
  if (maxScrollTop === 0 || !Number.isFinite(lineHeight) || lineHeight <= 0) return maxScrollTop;

  const firstLineTop = Number.isFinite(paddingTop) ? Math.max(0, paddingTop) : 0;
  if (maxScrollTop <= firstLineTop) return 0;

  const completeLines = Math.floor((maxScrollTop - firstLineTop) / lineHeight);
  return Math.min(maxScrollTop, firstLineTop + completeLines * lineHeight);
}
