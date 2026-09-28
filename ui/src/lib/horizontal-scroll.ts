/** Advance a viewport while retaining one complete item (a tab or table column).
 * If no overlap can make progress, page through oversized items without skipping their middle. */
export function horizontalPageTarget(
  offset: number,
  width: number,
  contentWidth: number,
  items: readonly { start: number; end: number }[],
  direction: -1 | 1,
): number {
  const visible = items.filter((item) => item.start >= offset - 1 && item.end <= offset + width + 1);
  const overlap = direction === 1 ? visible[visible.length - 1] : visible[0];
  const aligned = overlap && (direction === 1 ? overlap.start : overlap.end - width);
  const target = aligned !== undefined && (aligned - offset) * direction > 1
    ? aligned : offset + direction * width;
  return Math.max(0, Math.min(target, Math.max(0, contentWidth - width)));
}
