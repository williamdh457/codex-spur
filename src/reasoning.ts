export const CODEX_REASONING_EFFORTS = [
  "none",
  "minimal",
  "low",
  "medium",
  "high",
  "xhigh",
  "max",
  "ultra",
] as const;

export type CodexReasoningEffort = (typeof CODEX_REASONING_EFFORTS)[number];

export function deepSeekEffectiveEffort(effort: CodexReasoningEffort): "disabled" | "high" | "max" {
  if (effort === "none") return "disabled";
  if (effort === "xhigh" || effort === "max" || effort === "ultra") return "max";
  return "high";
}
