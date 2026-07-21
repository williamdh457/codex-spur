import { createHash } from "node:crypto";
import { readFile } from "node:fs/promises";
import { resolve } from "node:path";

const root = resolve(import.meta.dirname, "../..");
const manifest = JSON.parse(await readFile(resolve(root, "website/release-manifest.json"), "utf8"));
const packageJson = JSON.parse(await readFile(resolve(root, "package.json"), "utf8"));
const tauri = JSON.parse(await readFile(resolve(root, "src-tauri/tauri.conf.json"), "utf8"));
const cargo = await readFile(resolve(root, "src-tauri/Cargo.toml"), "utf8");
const expectedVersion = String(manifest.version).replace(/^v/, "");
const cargoVersion = cargo.match(/^version\s*=\s*"([^"]+)"/m)?.[1];

const assets = Array.isArray(manifest.assets) ? manifest.assets : [];
const mac = assets.find((a) => a.id === "macos-aarch64-dmg");
const win = assets.find((a) => a.id === "windows-x64-nsis");

const checks = [
  [packageJson.version === expectedVersion, `root package.json version ${packageJson.version} !== ${expectedVersion}`],
  [tauri.version === expectedVersion, `Tauri version ${tauri.version} !== ${expectedVersion}`],
  [cargoVersion === expectedVersion, `Cargo version ${cargoVersion} !== ${expectedVersion}`],
  [typeof manifest.releaseUrl === "string" && manifest.releaseUrl.includes(manifest.version), "releaseUrl must include version tag"],
  [assets.length >= 1, "release manifest must declare at least one asset"],
  [Boolean(mac), "missing macos-aarch64-dmg asset"],
  [Boolean(win), "missing windows-x64-nsis asset"],
];

if (mac) {
  checks.push(
    [mac.platform === "macOS", "mac asset platform must be macOS"],
    [mac.architecture === "Apple Silicon", "mac asset architecture must be Apple Silicon"],
    [mac.format === "DMG", "mac asset format must be DMG"],
    [typeof mac.sha256 === "string" && mac.sha256.length === 64 && /^[a-f0-9]+$/.test(mac.sha256), "mac sha256 must be lowercase SHA-256"],
    [typeof mac.assetUrl === "string" && mac.assetUrl.includes(`/releases/download/${manifest.version}/`), "mac assetUrl must point to declared release"],
  );
}
if (win) {
  const shaOk =
    typeof win.sha256 === "string" &&
    ((win.sha256.length === 64 && /^[a-f0-9]+$/.test(win.sha256)) || win.sha256 === "pending-windows-build");
  checks.push(
    [win.platform === "Windows", "windows asset platform must be Windows"],
    [win.architecture === "x64", "windows asset architecture must be x64"],
    [win.format === "NSIS", "windows asset format must be NSIS"],
    [shaOk, "windows sha256 must be lowercase SHA-256 or pending-windows-build"],
    [typeof win.assetUrl === "string" && win.assetUrl.includes(`/releases/download/${manifest.version}/`), "windows assetUrl must point to declared release"],
  );
}

const failed = checks.filter(([ok]) => !ok);
if (failed.length) {
  for (const [, message] of failed) console.error(`release validation failed: ${message}`);
  process.exit(1);
}

async function maybeVerifyLocal(path, expectedSha, label) {
  if (!expectedSha || expectedSha === "pending-windows-build") {
    console.log(`${label}: no local hash check (sha pending or empty).`);
    return;
  }
  try {
    const bytes = await readFile(path);
    const actual = createHash("sha256").update(bytes).digest("hex");
    if (actual !== expectedSha) {
      console.error(`release validation failed: ${label} SHA-256 ${actual} != manifest ${expectedSha}`);
      process.exit(1);
    }
    console.log(`${label} verified against local file (${actual}).`);
  } catch {
    console.log(`${label}: local file not present, skipping binary hash check.`);
  }
}

const localDmg = resolve(root, `src-tauri/target/release/bundle/dmg/Codex Spur_${expectedVersion}_aarch64.dmg`);
const localNsis = resolve(root, `src-tauri/target/release/bundle/nsis/Codex Spur_${expectedVersion}_x64-setup.exe`);
await maybeVerifyLocal(localDmg, mac?.sha256, "macOS DMG");
await maybeVerifyLocal(localNsis, win?.sha256, "Windows NSIS");
console.log(`Release manifest metadata verified (${manifest.version}, ${assets.length} assets).`);
