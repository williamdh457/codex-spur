export type SiteLocale = "zh-CN" | "en";

export interface SiteCopy {
  nav: Record<string, string>;
  hero: { kicker: string; title: string; description: string };
  workflow: Array<{ title: string; description: string }>;
  faq: Array<{ question: string; answer: string }>;
}

export interface ReleaseManifest {
  version: string;
  platform: "macOS";
  architecture: "Apple Silicon";
  releaseUrl: string;
  assetUrl: string;
  sha256: string;
}
