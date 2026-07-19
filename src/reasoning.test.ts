import { describe, expect, it } from "vitest";
import { CODEX_REASONING_EFFORTS, deepSeekEffectiveEffort } from "./reasoning";

describe("reasoning ladder", () => {
  it("keeps the complete Codex ladder", () => {
    expect(CODEX_REASONING_EFFORTS).toEqual([
      "none",
      "minimal",
      "low",
      "medium",
      "high",
      "xhigh",
      "max",
      "ultra",
    ]);
  });

  it("maps DeepSeek's two native levels without pretending there are more", () => {
    expect(deepSeekEffectiveEffort("none")).toBe("disabled");
    expect(deepSeekEffectiveEffort("minimal")).toBe("high");
    expect(deepSeekEffectiveEffort("high")).toBe("high");
    expect(deepSeekEffectiveEffort("xhigh")).toBe("max");
    expect(deepSeekEffectiveEffort("ultra")).toBe("max");
  });
});
