/** Map a series of points to SVG path coordinates. */
export type ChartPoint = { x: number; y: number };

/**
 * Catmull-Rom → cubic Bézier polyline. Falls back to straight segments
 * for fewer than two points. Does not animate or invent data.
 */
export function smoothLinePath(points: ChartPoint[]): string {
  if (points.length === 0) return "";
  const first = points[0]!;
  if (points.length === 1) {
    return `M ${first.x.toFixed(2)} ${first.y.toFixed(2)}`;
  }
  if (points.length === 2) {
    const second = points[1]!;
    return `M ${first.x.toFixed(2)} ${first.y.toFixed(2)} L ${second.x.toFixed(2)} ${second.y.toFixed(2)}`;
  }

  let path = `M ${first.x.toFixed(2)} ${first.y.toFixed(2)}`;
  for (let i = 0; i < points.length - 1; i += 1) {
    const p0 = points[Math.max(0, i - 1)]!;
    const p1 = points[i]!;
    const p2 = points[i + 1]!;
    const p3 = points[Math.min(points.length - 1, i + 2)]!;
    const cp1x = p1.x + (p2.x - p0.x) / 6;
    const cp1y = p1.y + (p2.y - p0.y) / 6;
    const cp2x = p2.x - (p3.x - p1.x) / 6;
    const cp2y = p2.y - (p3.y - p1.y) / 6;
    path += ` C ${cp1x.toFixed(2)} ${cp1y.toFixed(2)}, ${cp2x.toFixed(2)} ${cp2y.toFixed(2)}, ${p2.x.toFixed(2)} ${p2.y.toFixed(2)}`;
  }
  return path;
}

export function areaPath(
  linePath: string,
  first: ChartPoint,
  last: ChartPoint,
  baselineY: number,
): string {
  if (!linePath) return "";
  return `${linePath} L ${last.x.toFixed(2)} ${baselineY.toFixed(2)} L ${first.x.toFixed(2)} ${baselineY.toFixed(2)} Z`;
}
