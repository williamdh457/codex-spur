import { useEffect, useState } from "react";
import { getUsageDashboard } from "../api";
import type { UsageDashboardSnapshot, UsageRange } from "../types";
import { formatPercent, formatUsage, rangeLabel } from "./format";
import { UsageModelDonut } from "./ModelDonut";
import { UsageProviderBars } from "./ProviderBars";
import { UsageTrendChart } from "./TrendChart";

const RANGES: UsageRange[] = ["7d", "30d", "all"];

function UsageMetric({
  label,
  value,
  note,
}: {
  label: string;
  value: string;
  note: string;
}) {
  return (
    <div className="usage-metric">
      <span>{label}</span>
      <strong>{value}</strong>
      <small>{note}</small>
    </div>
  );
}

function errorMessage(error: unknown): string {
  return error instanceof Error ? error.message : String(error);
}

export function UsagePage() {
  const [range, setRange] = useState<UsageRange>("7d");
  const [usage, setUsage] = useState<UsageDashboardSnapshot | null>(null);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);
  const [reloadToken, setReloadToken] = useState(0);

  useEffect(() => {
    let active = true;
    void getUsageDashboard(range)
      .then((next) => {
        if (!active) return;
        setUsage(next);
        setError(null);
      })
      .catch((nextError: unknown) => {
        if (!active) return;
        setError(errorMessage(nextError));
      })
      .finally(() => {
        if (active) setLoading(false);
      });
    return () => {
      active = false;
    };
  }, [range, reloadToken]);

  const label = rangeLabel(range);

  const selectRange = (next: UsageRange) => {
    if (next === range) {
      setLoading(true);
      setReloadToken((token) => token + 1);
      return;
    }
    setLoading(true);
    setRange(next);
  };

  const refresh = () => {
    setLoading(true);
    setReloadToken((token) => token + 1);
  };

  return (
    <div className="page-stack usage-page">
      <section className="panel usage-panel">
        <div className="usage-toolbar">
          <div>
            <h2>本地代理用量</h2>
            <p>数据只来自本机代理，按日聚合，不会上传。</p>
          </div>
          <div className="usage-controls">
            <div className="segmented-control" role="group" aria-label="用量范围">
              {RANGES.map((item) => (
                <button
                  key={item}
                  type="button"
                  className={range === item ? "is-active" : ""}
                  aria-pressed={range === item}
                  onClick={() => selectRange(item)}
                >
                  {item === "7d" ? "7 天" : item === "30d" ? "30 天" : "全部"}
                </button>
              ))}
            </div>
            <button
              type="button"
              className="button button--secondary"
              onClick={refresh}
              disabled={loading}
            >
              {loading ? "读取中…" : "刷新"}
            </button>
          </div>
        </div>

        {error && (
          <div className="inline-warning" role="alert">
            读取用量失败：{error}
            <button type="button" className="link-button" onClick={refresh}>
              重试
            </button>
          </div>
        )}

        {!usage && loading && <div className="usage-loading">正在读取本地用量…</div>}

        {usage && (
          <>
            <div className="usage-meta">
              <span>
                {label}
                {loading ? " · 刷新中…" : ""}
              </span>
              <span>最后采样 {new Date(usage.sampledAt).toLocaleString("zh-CN")}</span>
            </div>

            <div className="usage-grid">
              <UsageMetric label="请求数" value={formatUsage(usage.requestCount)} note={label} />
              <UsageMetric label="总 Token" value={formatUsage(usage.totalTokens)} note="所选范围" />
              <UsageMetric
                label="输入 Token"
                value={formatUsage(usage.inputTokens)}
                note="本地代理记录"
              />
              <UsageMetric
                label="输出 Token"
                value={formatUsage(usage.outputTokens)}
                note="本地代理记录"
              />
              <UsageMetric
                label="失败率"
                value={formatPercent(usage.failureRate)}
                note={
                  usage.requestCount
                    ? `${formatUsage(usage.failedRequests)} 次失败`
                    : "无请求"
                }
              />
              <UsageMetric
                label="缓存命中率"
                value={formatPercent(usage.cacheHitRate)}
                note={usage.cacheHitRate === null ? "暂无有效观察值" : "有效观察值"}
              />
            </div>

            <div className="usage-dashboard-grid">
              <section className="usage-card">
                <div className="usage-card__header">
                  <div>
                    <h3>Token 趋势</h3>
                    <p>输入、输出、总量、失败请求与缓存命中率</p>
                  </div>
                </div>
                <UsageTrendChart points={usage.trend} />
              </section>

              <section className="usage-card">
                <div className="usage-card__header">
                  <div>
                    <h3>模型用量</h3>
                    <p>按总 Token 降序</p>
                  </div>
                </div>
                <UsageModelDonut models={usage.models} total={usage.selectedRangeTokens} />
              </section>

              <section className="usage-card usage-card--wide">
                <div className="usage-card__header">
                  <div>
                    <h3>供应商分布</h3>
                    <p>不显示账号、凭证或秘密信息</p>
                  </div>
                </div>
                <UsageProviderBars providers={usage.providers} />
              </section>
            </div>

            <div className="usage-notes">
              <p>Token 以本地代理记录的上游 usage 为准；没有上游 usage 时使用现有估算逻辑。</p>
              <p>当前按日聚合，不提供小时级趋势；缓存命中率无有效观察值时显示“暂无数据”。</p>
            </div>
          </>
        )}
      </section>
    </div>
  );
}
