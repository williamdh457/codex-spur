import { createHash } from "node:crypto";
import { readFile } from "node:fs/promises";
import { resolve } from "node:path";

const root = resolve(import.meta.dirname, "../..");
const manifest = JSON.parse(await readFile(resolve(root, "website/release-manifest.json"), "utf8"));
const packageJson = JSON.parse(await readFile(resolve(root, "package.json"), "utf8"));
const tauri = JSON.parse(await readFile(resolve(root, "src-tauri/tauri.conf.json"), "utf8"));
const cargo = await readFile(resolve(root, "src-tauri/Cargo.toml"), "utf8");
const expectedVersion = manifest.version.replace(/^v/, "");
const cargoVersion = cargo.match(/^version\s*=\s*"([^"]+)"/m)?.[1];
const checks = [
  [packageJson.version === expectedVersion, `root package.json version ${packageJson.version} !== ${expectedVersion}`],
  [tauri.version === expectedVersion, `Tauri version ${tauri.version} !== ${expectedVersion}`],
  [cargoVersion === expectedVersion, `Cargo version ${cargoVersion} !== ${expectedVersion}`],
  [manifest.platform === "macOS", "release platform must be macOS"],
  [manifest.architecture === "Apple Silicon", "release architecture must be Apple Silicon"],
  [manifest.sha256.length === 64 && /^[a-f0-9]+$/.test(manifest.sha256), "release sha256 must be a lowercase SHA-256"],
  [manifest.assetUrl.includes(`/releases/download/${manifest.version}/`), "asset URL must point to the declared GitHub release"],
];
const failed = checks.filter(([ok]) => !ok);
if (failed.length) {
  for (const [, message] of failed) console.error(`release validation failed: ${message}`);
  process.exit(1);
}
const localDmg = resolve(root, "src-tauri/target/release/bundle/dmg/Codex Spur_0.1.0_aarch64.dmg");
try {
  const bytes = await readFile(localDmg);
  const actual = createHash("sha256").update(bytes).digest("hex");
  if (actual !== manifest.sha256) {
    console.error(`release validation failed: local DMG SHA-256 ${actual} != manifest ${manifest.sha256}`);
    process.exit(1);
  }
  console.log(`Release manifest verified against local DMG (${manifest.version}, ${actual}).`);
} catch {
  console.log(`Release manifest metadata verified (${manifest.version}); local DMG not present, skipping binary hash check.`);
}
