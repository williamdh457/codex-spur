import { useMemo, useState } from "react";
import type { UsageTrendPoint } from "../types";
import { areaPath, smoothLinePath } from "./chartPaths";
import { formatPercent, formatUsage } from "./format";

const WIDTH = 760;
const HEIGHT = 260;
const PAD = { left: 48, right: 48, top: 22, bottom: 36 };

type Props = {
  points: UsageTrendPoint[];
};

export function UsageTrendChart({ points }: Props) {
  const [hover, setHover] = useState<number | null>(null);

  const geometry = useMemo(() => {
    if (!points.length) return null;
    const maxTokens = Math.max(1, ...points.map((p) => p.totalTokens));
    const maxFailed = Math.max(1, ...points.map((p) => p.failedRequests));
    const span = Math.max(1, points.length - 1);
    const xAt = (index: number) => PAD.left + (index / span) * (WIDTH - PAD.left - PAD.right);
    const yToken = (value: number) =>
      PAD.top + (1 - value / maxTokens) * (HEIGHT - PAD.top - PAD.bottom);
    const yFailed = (value: number) =>
      PAD.top + (1 - value / maxFailed) * (HEIGHT - PAD.top - PAD.bottom);
    const yCache = (rate: number | null) =>
      rate === null
        ? null
        : PAD.top + (1 - Math.min(1, Math.max(0, rate))) * (HEIGHT - PAD.top - PAD.bottom);
    const baseline = HEIGHT - PAD.bottom;

    const series = (key: "inputTokens" | "outputTokens" | "totalTokens") =>
      points.map((point, index) => ({ x: xAt(index), y: yToken(point[key]) }));
    const failedPts = points.map((point, index) => ({
      x: xAt(index),
      y: yFailed(point.failedRequests),
    }));
    const cachePts = points
      .map((point, index) => {
        const y = yCache(point.cacheHitRate);
        return y === null ? null : { x: xAt(index), y };
      })
      .filter((p): p is { x: number; y: number } => p !== null);

    const totalLine = series("totalTokens");
    const totalFirst = totalLine[0]!;
    const totalLast = totalLine[totalLine.length - 1]!;
    const totalPath = smoothLinePath(totalLine);
    const cacheFirst = cachePts[0];
    return {
      maxTokens,
      maxFailed,
      xAt,
      yToken,
      baseline,
      inputPath: smoothLinePath(series("inputTokens")),
      outputPath: smoothLinePath(series("outputTokens")),
      totalPath,
      totalArea: areaPath(totalPath, totalFirst, totalLast, baseline),
      failedPath: smoothLinePath(failedPts),
      cachePath:
        cachePts.length >= 2
          ? smoothLinePath(cachePts)
          : cacheFirst
            ? `M ${cacheFirst.x.toFixed(2)} ${cacheFirst.y.toFixed(2)}`
            : "",
      hasCache: cachePts.length > 0,
    };
  }, [points]);

  if (!points.length || !geometry) {
    return <div className="usage-empty">所选范围暂无用量记录</div>;
  }

  const firstDay = points[0]!.day;
  const lastDay = points[points.length - 1]!.day;
  const active = hover !== null ? points[hover] ?? null : null;
  const tipX = hover !== null ? geometry.xAt(hover) : 0;

  return (
    <div className="usage-chart-wrap">
      <div className="usage-legend" aria-hidden="true">
        <span><i className="legend-dot legend-dot--input" />输入</span>
        <span><i className="legend-dot legend-dot--output" />输出</span>
        <span><i className="legend-dot legend-dot--total" />总计</span>
        <span><i className="legend-dot legend-dot--failed" />失败请求</span>
        {geometry.hasCache && (
          <span><i className="legend-dot legend-dot--cache" />缓存命中率</span>
        )}
      </div>
      <div className="usage-trend-frame">
        <svg
          className="usage-trend"
          viewBox={`0 0 ${WIDTH} ${HEIGHT}`}
          role="img"
          aria-label="按日 Token 用量趋势"
          onMouseLeave={() => setHover(null)}
        >
          <title>按日 Token 用量趋势</title>
          <desc>
            显示输入、输出、总 Token、失败请求以及缓存命中率的日趋势。悬停数据点可查看明细。
          </desc>

          {[0, 0.25, 0.5, 0.75, 1].map((ratio) => (
            <line
              key={ratio}
              x1={PAD.left}
              x2={WIDTH - PAD.right}
              y1={geometry.yToken(geometry.maxTokens * ratio)}
              y2={geometry.yToken(geometry.maxTokens * ratio)}
              className="usage-gridline"
            />
          ))}

          <text x={4} y={PAD.top + 4} className="usage-axis-label">Token</text>
          <text x={WIDTH - 4} y={PAD.top + 4} textAnchor="end" className="usage-axis-label">
            缓存 %
          </text>
          <text x={4} y={HEIGHT - PAD.bottom} className="usage-axis-label">0</text>
          <text x={4} y={PAD.top + 14} className="usage-axis-label">
            {formatUsage(geometry.maxTokens)}
          </text>
          <text x={WIDTH - 4} y={PAD.top + 14} textAnchor="end" className="usage-axis-label">
            100%
          </text>
          <text x={WIDTH - 4} y={HEIGHT - PAD.bottom} textAnchor="end" className="usage-axis-label">
            0%
          </text>

          <path d={geometry.totalArea} className="usage-area" />
          <path d={geometry.inputPath} className="usage-line usage-line--input" />
          <path d={geometry.outputPath} className="usage-line usage-line--output" />
          <path d={geometry.totalPath} className="usage-line usage-line--total" />
          <path d={geometry.failedPath} className="usage-line usage-line--failed" />
          {geometry.cachePath && (
            <path d={geometry.cachePath} className="usage-line usage-line--cache" />
          )}

          {points.map((point, index) => {
            const cx = geometry.xAt(index);
            const cy = geometry.yToken(point.totalTokens);
            return (
              <g key={point.day}>
                <circle
                  cx={cx}
                  cy={cy}
                  r={hover === index ? 4.5 : 3}
                  className={`usage-point${hover === index ? " is-active" : ""}`}
                />
                {/* Invisible hit target for hover */}
                <rect
                  x={cx - Math.max(8, (WIDTH - PAD.left - PAD.right) / Math.max(points.length, 1) / 2)}
                  y={PAD.top}
                  width={Math.max(16, (WIDTH - PAD.left - PAD.right) / Math.max(points.length, 1))}
                  height={HEIGHT - PAD.top - PAD.bottom}
                  fill="transparent"
                  onMouseEnter={() => setHover(index)}
                />
              </g>
            );
          })}

          {hover !== null && (
            <line
              x1={tipX}
              x2={tipX}
              y1={PAD.top}
              y2={HEIGHT - PAD.bottom}
              className="usage-hover-line"
            />
          )}

          <text x={PAD.left} y={HEIGHT - 10} className="usage-axis-label">
            {firstDay}
          </text>
          <text
            x={WIDTH - PAD.right}
            y={HEIGHT - 10}
            textAnchor="end"
            className="usage-axis-label"
          >
            {lastDay}
          </text>
        </svg>

        {active && hover !== null && (
          <div
            className="usage-tooltip"
            style={{
              left: `${(tipX / WIDTH) * 100}%`,
            }}
            role="tooltip"
          >
            <strong>{active.day}</strong>
            <span>请求 {formatUsage(active.requestCount)}</span>
            <span>输入 {formatUsage(active.inputTokens)}</span>
            <span>输出 {formatUsage(active.outputTokens)}</span>
            <span>总计 {formatUsage(active.totalTokens)}</span>
            <span>失败 {formatUsage(active.failedRequests)}</span>
            <span>缓存 {formatPercent(active.cacheHitRate)}</span>
          </div>
        )}
      </div>
    </div>
  );
}
