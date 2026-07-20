import { describe, expect, it } from "vitest";
import { areaPath, smoothLinePath } from "./chartPaths";

describe("chart path helpers", () => {
  it("handles empty and single points", () => {
    expect(smoothLinePath([])).toBe("");
    expect(smoothLinePath([{ x: 1, y: 2 }])).toContain("M 1.00 2.00");
  });

  it("builds straight and smooth paths", () => {
    const two = smoothLinePath([
      { x: 0, y: 0 },
      { x: 10, y: 10 },
    ]);
    expect(two).toContain("L");
    const multi = smoothLinePath([
      { x: 0, y: 10 },
      { x: 10, y: 0 },
      { x: 20, y: 10 },
      { x: 30, y: 5 },
    ]);
    expect(multi.startsWith("M ")).toBe(true);
    expect(multi).toContain("C ");
  });

  it("closes an area under a line", () => {
    const line = smoothLinePath([
      { x: 0, y: 5 },
      { x: 10, y: 2 },
    ]);
    const area = areaPath(line, { x: 0, y: 5 }, { x: 10, y: 2 }, 20);
    expect(area.endsWith("Z")).toBe(true);
    expect(area).toContain("20.00");
  });
});
