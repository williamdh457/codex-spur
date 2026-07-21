import manifest from "../release-manifest.json";
import type { ReleaseAsset, ReleaseManifest } from "./types";

export const release = manifest as ReleaseManifest;

export function assetById(id: string): ReleaseAsset | undefined {
  return release.assets.find((asset) => asset.id === id);
}

export const macAsset = assetById("macos-aarch64-dmg");
export const windowsAsset = assetById("windows-x64-nsis");

/** Primary download for nav/hero defaults to mac when present, else first asset. */
export const primaryAsset: ReleaseAsset =
  macAsset ?? windowsAsset ?? release.assets[0];
