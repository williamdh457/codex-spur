import { describe, expect, it } from "vitest";
import { formatPercent, formatUsage, rangeLabel } from "./format";

describe("usage formatters", () => {
  it("formats integers with zh-CN grouping", () => {
    expect(formatUsage(0)).toBe("0");
    expect(formatUsage(1234)).toMatch(/1[,.]?234|1,234/);
    expect(formatUsage(1_000_000)).toContain("000");
  });

  it("formats percents and null as 暂无数据", () => {
    expect(formatPercent(null)).toBe("暂无数据");
    expect(formatPercent(undefined)).toBe("暂无数据");
    expect(formatPercent(0)).toBe("0.0%");
    expect(formatPercent(0.125)).toBe("12.5%");
    expect(formatPercent(1)).toBe("100.0%");
  });

  it("labels ranges in Chinese", () => {
    expect(rangeLabel("7d")).toBe("最近 7 天");
    expect(rangeLabel("30d")).toBe("最近 30 天");
    expect(rangeLabel("all")).toBe("全部历史");
  });
});
