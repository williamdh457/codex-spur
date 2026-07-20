import type { UsageBreakdown } from "../types";
import { formatPercent, formatUsage } from "./format";

type Props = {
  providers: UsageBreakdown[];
};

export function UsageProviderBars({ providers }: Props) {
  if (!providers.length) {
    return <div className="usage-empty">暂无供应商用量</div>;
  }

  const maxShare = Math.max(1e-9, ...providers.map((p) => p.tokenShare));

  return (
    <div className="usage-providers" role="list" aria-label="供应商 Token 分布">
      {providers.map((provider) => {
        const hasFailures = provider.failedRequests > 0;
        const widthPct = Math.max(2, (provider.tokenShare / maxShare) * 100);
        return (
          <div
            className={`usage-provider-row${hasFailures ? " usage-provider-row--failed" : ""}`}
            role="listitem"
            key={provider.name}
          >
            <div className="usage-provider-head">
              <strong title={provider.name}>{provider.name}</strong>
              <span>
                {formatUsage(provider.totalTokens)} Token · {formatUsage(provider.requestCount)} 次 ·
                占比 {formatPercent(provider.tokenShare)}
                {hasFailures
                  ? ` · 失败 ${formatUsage(provider.failedRequests)}`
                  : " · 无失败"}
              </span>
            </div>
            <div
              className="usage-provider-bar"
              aria-label={`${provider.name} Token 占比 ${formatPercent(provider.tokenShare)}${
                hasFailures ? `，失败 ${formatUsage(provider.failedRequests)} 次` : ""
              }`}
            >
              <span
                className={hasFailures ? "is-failed" : undefined}
                style={{ width: `${widthPct}%` }}
              />
            </div>
          </div>
        );
      })}
    </div>
  );
}
