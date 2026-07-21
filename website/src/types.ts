export type SiteLocale = "zh-CN" | "en";

export type SiteCopy = {
  nav: {
    product: string;
    workflow: string;
    security: string;
    faq: string;
    github: string;
    download: string;
  };
  hero: {
    kicker: string;
    title: string;
    description: string;
  };
  workflow: Array<{ title: string; description: string }>;
  faq: Array<{ question: string; answer: string }>;
};

export type ReleaseAsset = {
  id: string;
  platform: "macOS" | "Windows";
  architecture: string;
  format: "DMG" | "NSIS";
  assetUrl: string;
  sha256: string;
};

export type ReleaseManifest = {
  version: string;
  releaseUrl: string;
  assets: ReleaseAsset[];
};
