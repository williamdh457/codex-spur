/** Format integer token/request counts with zh-CN grouping. */
export function formatUsage(value: number): string {
  return new Intl.NumberFormat("zh-CN").format(value);
}

/** Format a 0–1 ratio as percent, or “暂无数据” when null. */
export function formatPercent(value: number | null | undefined): string {
  if (value === null || value === undefined || Number.isNaN(value)) {
    return "暂无数据";
  }
  return `${(value * 100).toFixed(1)}%`;
}

export function rangeLabel(range: "7d" | "30d" | "all"): string {
  if (range === "7d") return "最近 7 天";
  if (range === "30d") return "最近 30 天";
  return "全部历史";
}
