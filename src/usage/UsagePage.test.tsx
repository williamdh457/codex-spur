import { cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { UsageDashboardSnapshot, UsageRange } from "../types";
import { UsageModelDonut } from "./ModelDonut";
import { UsagePage } from "./UsagePage";
import { UsageProviderBars } from "./ProviderBars";
import { UsageTrendChart } from "./TrendChart";

const getUsageDashboard = vi.fn<(range: UsageRange) => Promise<UsageDashboardSnapshot>>();

vi.mock("../api", () => ({
  getUsageDashboard: (range: UsageRange) => getUsageDashboard(range),
}));

function sampleDashboard(overrides: Partial<UsageDashboardSnapshot> = {}): UsageDashboardSnapshot {
  return {
    range: "7d",
    requestCount: 12,
    inputTokens: 1000,
    outputTokens: 500,
    totalTokens: 1500,
    todayTokens: 200,
    selectedRangeTokens: 1500,
    failedRequests: 2,
    failureRate: 2 / 12,
    cacheHitRate: 0.4,
    sampledAt: Date.UTC(2026, 0, 15, 8, 0, 0),
    trend: [
      {
        day: "2026-01-14",
        requestCount: 5,
        inputTokens: 400,
        outputTokens: 200,
        totalTokens: 600,
        failedRequests: 1,
        cacheHitRate: 0.5,
      },
      {
        day: "2026-01-15",
        requestCount: 7,
        inputTokens: 600,
        outputTokens: 300,
        totalTokens: 900,
        failedRequests: 1,
        cacheHitRate: 0.3,
      },
    ],
    models: [
      {
        name: "gpt-large-very-long-model-id-for-truncation",
        requestCount: 8,
        inputTokens: 800,
        outputTokens: 400,
        totalTokens: 1200,
        failedRequests: 1,
        tokenShare: 0.8,
      },
      {
        name: "gpt-small",
        requestCount: 4,
        inputTokens: 200,
        outputTokens: 100,
        totalTokens: 300,
        failedRequests: 0,
        tokenShare: 0.2,
      },
    ],
    providers: [
      {
        name: "Alpha",
        requestCount: 8,
        inputTokens: 800,
        outputTokens: 400,
        totalTokens: 1200,
        failedRequests: 2,
        tokenShare: 0.8,
      },
      {
        name: "Beta",
        requestCount: 4,
        inputTokens: 200,
        outputTokens: 100,
        totalTokens: 300,
        failedRequests: 0,
        tokenShare: 0.2,
      },
    ],
    ...overrides,
  };
}

describe("UsagePage", () => {
  beforeEach(() => {
    getUsageDashboard.mockReset();
  });

  afterEach(() => {
    cleanup();
  });

  it("shows loading on first load", async () => {
    let resolve!: (value: UsageDashboardSnapshot) => void;
    getUsageDashboard.mockReturnValue(
      new Promise<UsageDashboardSnapshot>((r) => {
        resolve = r;
      }),
    );
    render(<UsagePage />);
    expect(screen.getByText("正在读取本地用量…")).toBeInTheDocument();
    resolve(sampleDashboard());
    await waitFor(() => {
      expect(screen.getByText("本地代理用量")).toBeInTheDocument();
      expect(screen.getByText("请求数")).toBeInTheDocument();
    });
  });

  it("shows readable error and retries", async () => {
    getUsageDashboard
      .mockRejectedValueOnce(new Error("disk locked"))
      .mockResolvedValueOnce(sampleDashboard());
    render(<UsagePage />);
    await waitFor(() => {
      expect(screen.getByRole("alert")).toHaveTextContent("disk locked");
    });
    fireEvent.click(screen.getByRole("button", { name: "重试" }));
    await waitFor(() => {
      expect(screen.getByText("请求数")).toBeInTheDocument();
      expect(screen.queryByRole("alert")).not.toBeInTheDocument();
    });
    expect(getUsageDashboard).toHaveBeenCalledTimes(2);
  });

  it("switches ranges with correct IPC parameter", async () => {
    getUsageDashboard.mockImplementation((range: UsageRange) =>
      Promise.resolve(
        sampleDashboard({
          range,
          requestCount: range === "30d" ? 30 : range === "all" ? 99 : 7,
        }),
      ),
    );
    render(<UsagePage />);
    await waitFor(() => expect(getUsageDashboard).toHaveBeenCalledWith("7d"));

    fireEvent.click(screen.getByRole("button", { name: "30 天" }));
    await waitFor(() => expect(getUsageDashboard).toHaveBeenLastCalledWith("30d"));

    fireEvent.click(screen.getByRole("button", { name: "全部" }));
    await waitFor(() => expect(getUsageDashboard).toHaveBeenLastCalledWith("all"));
  });

  it("formats summary cards including 暂无数据", async () => {
    getUsageDashboard.mockResolvedValue(
      sampleDashboard({
        requestCount: 0,
        failureRate: null,
        cacheHitRate: null,
        totalTokens: 0,
        inputTokens: 0,
        outputTokens: 0,
        failedRequests: 0,
        models: [],
        providers: [],
        trend: [],
      }),
    );
    render(<UsagePage />);
    await waitFor(() => {
      expect(screen.getAllByText("暂无数据").length).toBeGreaterThanOrEqual(2);
    });
    expect(screen.getByText("无请求")).toBeInTheDocument();
    expect(screen.getByText("暂无有效观察值")).toBeInTheDocument();
  });

  it("keeps previous data visible while refreshing", async () => {
    getUsageDashboard.mockResolvedValueOnce(sampleDashboard({ requestCount: 12 }));
    render(<UsagePage />);
    await waitFor(() => expect(screen.getByText("请求数")).toBeInTheDocument());

    let resolveRefresh!: (value: UsageDashboardSnapshot) => void;
    getUsageDashboard.mockReturnValueOnce(
      new Promise<UsageDashboardSnapshot>((r) => {
        resolveRefresh = r;
      }),
    );
    fireEvent.click(screen.getByRole("button", { name: "刷新" }));
    expect(screen.getByText(/刷新中/)).toBeInTheDocument();
    // previous metrics still present
    expect(screen.getByText("请求数")).toBeInTheDocument();
    resolveRefresh(sampleDashboard({ requestCount: 20 }));
    await waitFor(() => expect(screen.queryByText(/刷新中/)).not.toBeInTheDocument());
  });
});

describe("UsageTrendChart", () => {
  afterEach(() => cleanup());

  it("renders empty state without inventing series", () => {
    render(<UsageTrendChart points={[]} />);
    expect(screen.getByText("所选范围暂无用量记录")).toBeInTheDocument();
  });

  it("renders accessible trend chart with series", () => {
    const { container } = render(
      <UsageTrendChart points={sampleDashboard().trend} />,
    );
    const svg = container.querySelector("svg.usage-trend");
    expect(svg).toHaveAttribute("role", "img");
    expect(svg).toHaveAttribute("aria-label", "按日 Token 用量趋势");
    expect(container.querySelector("title")?.textContent).toContain("Token");
    expect(container.querySelectorAll("path.usage-line").length).toBeGreaterThanOrEqual(3);
  });
});

describe("UsageModelDonut", () => {
  afterEach(() => cleanup());

  it("shows empty state when no model tokens", () => {
    render(<UsageModelDonut models={[]} total={0} />);
    expect(screen.getByText("所选范围暂无模型用量")).toBeInTheDocument();
  });

  it("keeps model order and exposes full name via title", () => {
    const models = sampleDashboard().models;
    render(<UsageModelDonut models={models} total={1500} />);
    const names = screen.getAllByTitle(/gpt-/).map((el) => el.getAttribute("title"));
    expect(names[0]).toBe(models[0]!.name);
    expect(names[1]).toBe(models[1]!.name);
    // failure count expressed in text, not only color
    expect(screen.getByText(/失败 1/)).toBeInTheDocument();
    expect(screen.getByText(/失败 0/)).toBeInTheDocument();
  });
});

describe("UsageProviderBars", () => {
  afterEach(() => cleanup());

  it("renders provider share text and failure copy without color-only cues", () => {
    render(<UsageProviderBars providers={sampleDashboard().providers} />);
    expect(screen.getByText(/占比 80\.0%/)).toBeInTheDocument();
    expect(screen.getByText(/失败 2/)).toBeInTheDocument();
    expect(screen.getByText(/无失败/)).toBeInTheDocument();
    const failedRow = document.querySelector(".usage-provider-row--failed");
    expect(failedRow).not.toBeNull();
    expect(failedRow?.textContent).toMatch(/失败 2/);
  });
});
