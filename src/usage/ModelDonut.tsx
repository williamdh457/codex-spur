import type { UsageBreakdown } from "../types";
import { formatPercent, formatUsage } from "./format";

const COLORS = ["#7c9cff", "#59c3c3", "#d8a657", "#cf7d9d", "#8bcf78", "#9b8cff", "#e09b6d"];

type Props = {
  models: UsageBreakdown[];
  total: number;
};

export function UsageModelDonut({ models, total }: Props) {
  if (!models.length || total === 0) {
    return <div className="usage-empty">所选范围暂无模型用量</div>;
  }

  const radius = 58;
  const circumference = 2 * Math.PI * radius;
  const segments = models.map((model, index) => {
    const dash = Math.max(0, model.tokenShare) * circumference;
    const offset = models
      .slice(0, index)
      .reduce((sum, item) => sum + Math.max(0, item.tokenShare) * circumference, 0);
    return { model, index, dash, offset };
  });

  return (
    <div className="usage-donut-layout">
      <div className="usage-donut" role="img" aria-label={`模型 Token 占比，合计 ${formatUsage(total)}`}>
        <svg viewBox="0 0 150 150">
          <title>模型 Token 占比</title>
          <desc>按模型总 Token 绘制的圆环图，颜色循环使用固定调色板。</desc>
          <circle cx="75" cy="75" r={radius} className="usage-donut-track" />
          {segments.map(({ model, index, dash, offset }) => (
            <circle
              key={model.name}
              cx="75"
              cy="75"
              r={radius}
              stroke={COLORS[index % COLORS.length]}
              strokeDasharray={`${dash} ${Math.max(0, circumference - dash)}`}
              strokeDashoffset={-offset}
              className="usage-donut-segment"
            >
              <title>
                {model.name} · {formatPercent(model.tokenShare)} · {formatUsage(model.totalTokens)} Token
              </title>
            </circle>
          ))}
        </svg>
        <div>
          <strong>{formatUsage(total)}</strong>
          <span>总 Token</span>
        </div>
      </div>

      <div className="usage-breakdown-table" role="table" aria-label="模型用量明细">
        <div className="usage-breakdown-row usage-breakdown-row--head" role="row">
          <span role="columnheader">模型</span>
          <span role="columnheader">请求</span>
          <span role="columnheader">Token</span>
          <span role="columnheader">占比</span>
          <span role="columnheader">失败</span>
        </div>
        {models.map((model, index) => (
          <div className="usage-breakdown-row" role="row" key={model.name}>
            <span className="usage-name" title={model.name} role="cell">
              <i
                className="usage-swatch"
                style={{ background: COLORS[index % COLORS.length] }}
                aria-hidden="true"
              />
              {model.name}
            </span>
            <span role="cell">{formatUsage(model.requestCount)}</span>
            <strong role="cell">{formatUsage(model.totalTokens)}</strong>
            <span role="cell">{formatPercent(model.tokenShare)}</span>
            <span
              role="cell"
              className={model.failedRequests > 0 ? "usage-failure" : undefined}
            >
              失败 {formatUsage(model.failedRequests)}
            </span>
          </div>
        ))}
      </div>
    </div>
  );
}
