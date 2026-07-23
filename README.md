<p align="center">
  <img src="src/assets/codex-spur-icon.png" alt="Codex Spur" width="128" height="128">
</p>

<h1 align="center">Codex Spur</h1>

<p align="center">
  <b>English</b> · <a href="./README.zh-CN.md">中文</a>
</p>

<h2 align="center">
  All your models. One Codex picker.<br>
  One click to switch.
</h2>

<p align="center">
  <em style="font-size: 1.15em; line-height: 1.55;">
    Connect Kimi, DeepSeek, xAI, OpenAI multi-account, or any compatible gateway once — then flip between them in the <strong>Codex / ChatGPT Desktop model menu</strong>, the same place you pick official models. No extra tabs. No API gymnastics mid-flow.
  </em>
</p>

<p align="center">
  <a href="https://github.com/williamdh457/codex-spur/releases/latest">Download DMG</a>
  ·
  <a href="#if-macos-says-app-is-damaged">“App is damaged?” Fix</a>
  ·
  <a href="./CHANGELOG.md">Changelog</a>
  ·
  <a href="./LICENSE">MIT License</a>
</p>

---

> [!IMPORTANT]
> ### 首次打开提示 **「应用已损坏，无法打开」**？
>
> **几乎不是安装包坏了。** 公开 Release 的 DMG 是 **ad-hoc 签名、未做 Apple 公证**。浏览器下载会带隔离属性，Gatekeeper 常误报为「已损坏」。
>
> **最快处理**（先拖进「应用程序」）：
>
> ```bash
> xattr -cr "/Applications/Codex Spur.app"
> open "/Applications/Codex Spur.app"
> ```
>
> 或：对 App **右键 → 打开 → 打开** · 或 **系统设置 → 隐私与安全性 → 仍要打开**。
>
> 完整说明：[§ If macOS says “app is damaged”](#if-macos-says-app-is-damaged) · 中文版：[README.zh-CN.md](./README.zh-CN.md#首次打开提示应用已损坏)
>
> ---
>
> ### If macOS says **“Codex Spur is damaged and can’t be opened”**
>
> **The download is almost never corrupt.** Public Release DMGs are **ad-hoc signed and not notarized**, so Gatekeeper treats the quarantine flag from a browser download as “damaged.”
>
> **Fastest fix** (after dragging the app into Applications):
>
> ```bash
> xattr -cr "/Applications/Codex Spur.app"
> open "/Applications/Codex Spur.app"
> ```
>
> Or: **right-click the app → Open → Open** · or **System Settings → Privacy & Security → Open Anyway**.
>
> Full options: [§ If macOS says “app is damaged”](#if-macos-says-app-is-damaged)

---

## About

### One Codex picker for every model you configured

That is the whole point.

<p align="center">
  <img src="docs/images/codex-model-picker.png" alt="Codex model picker with Grok, Kimi, DeepSeek, and OpenAI models published by Codex Spur" width="720">
</p>

<p align="center"><sub>Your configured models in the native Codex / ChatGPT Desktop picker — one click to switch.</sub></p>

You wire providers in Spur, enable the routes you care about, hit **Review & Apply** — and they show up in the **native Codex model picker**. Coding on Kimi for speed, DeepSeek for cost, OpenAI for the hard pass, a custom gateway for a private endpoint: **switch with one click**, without leaving the app or rewriting configs.

Spur is a **local-first** control surface for the models you actually ship with — not a cloud locker, not a ChatGPT.app patcher.

**Keys stay on your Mac.** API keys, session tokens, refresh tokens, and proxy bearers never leave this machine: encrypted at rest, never exposed to the UI layer, never uploaded to a Codex Spur cloud. No credential telemetry.

**No app injection.** Integration uses only supported seams:

1. a localhost OpenAI Responses–compatible proxy  
2. a generated `model_catalog_json`  
3. a dedicated provider id: `codex_select`  

Closing the main window keeps the menu-bar proxy alive. Quitting the app stops the proxy and releases account leases. Version 1 does **not** install a LaunchAgent or privileged helper.

| | |
|---|---|
| Platform | macOS (Apple Silicon) · Windows x64 |
| Stack | Tauri 2 · React · TypeScript · Rust |
| Version | **0.1.5** |
| License | MIT |

---

## Features

### Provider instances

- Add unlimited instances of the same kind (several OpenAI, several Kimi, …)
- **Add → save & fetch models → a new row on Overview**
- OpenAI entry methods: official browser OAuth (PKCE), API key, multi-account credentials JSON, provider config JSON
- Kimi Code defaults to `https://api.kimi.com/coding/v1`
- OpenCode Go imports the local `opencode-go` API credential from `$XDG_DATA_HOME/opencode/auth.json` (fallback `~/.local/share/opencode/auth.json`) or accepts a manually entered key, and uses `https://opencode.ai/zen/go/v1`
- Fetched models stay **candidates** until you enable them on the Models page

### Routing & scheduling

Multi-account OpenAI instances support:

- `Pool` — load-aware pool selection  
- `Fixed` — pin one account  

Pool order (independent implementation of an observable contract):

1. `previous_response_id` affinity  
2. session-hash affinity  
3. filtered, load-aware Top-K weighted pick  

Accounts must pass capability, token, cooldown, quota, and concurrency checks. Sticky bindings escape when unhealthy.

### Reasoning ladder

Every route maps all eight Codex levels:

```text
none · minimal · low · medium · high · xhigh · max · ultra
```

If an upstream model cannot vary reasoning, Spur says so honestly instead of faking distinct behavior.

### Quota & reset credits

- Nearest **5-hour / 7-day** windows by `limit_window_seconds`
- Quota refresh failures do not auto-disable a healthy account
- Reset-credit spend requires confirmation, an idempotency key, and an audit trail

### Security

- Secrets are local-only  
- Frontend never receives raw access tokens, refresh tokens, API keys, or proxy bearers  
- SQLite stores AES-256-GCM ciphertext; master key is `master_key.hex` (`0600`) under the app data dir  
- Logs and UI errors redact tokens, emails, and Authorization material  

Typical data directory:

```text
~/Library/Application Support/com.codexspur.desktop/
```

---

## Install

### Requirements

- **macOS Apple Silicon** (`aarch64` DMG) and/or **Windows x64** (NSIS setup `.exe`)
- ChatGPT Desktop / Codex installed and reading the user Codex home (third-party rows in the GUI usually need a valid Desktop login—see Desktop visibility):
  - macOS: `~/.codex`
  - Windows: `%USERPROFILE%\.codex`
- Network access to the upstream APIs you configure

### From Release

**macOS**

1. Open the [latest Release](https://github.com/williamdh457/codex-spur/releases/latest) and download  
   `Codex.Spur_0.1.5_aarch64.dmg` (GitHub may normalize spaces in the asset name)
2. Open the DMG and drag **Codex Spur** into **Applications**
3. Open the app (see [“app is damaged”](#if-macos-says-app-is-damaged) if macOS blocks the first launch)
4. Leave the menu-bar process running while you use Spur-backed models

**Windows**

1. Download `Codex.Spur_<version>_x64-setup.exe` from the same Release
2. Run the NSIS installer (current-user install)
3. If **SmartScreen** warns about an unknown publisher, choose **More info → Run anyway** (v1 builds are unsigned, same policy as the unsigned mac DMG)
4. Launch **Codex Spur**, configure providers, then **Review & Apply** so writes land in `%USERPROFILE%\.codex`
5. Leave the tray process running while you use Spur-backed models

Windows support depends on ChatGPT / Codex Desktop already being installed and reading `%USERPROFILE%\.codex`. Custom `model_providers` behavior may differ from macOS; treat third-party model visibility as best-effort.

### If macOS says “app is damaged”

macOS may show one of:

- *“Codex Spur” is damaged and can’t be opened. You should move it to the Trash.*
- *Apple cannot check it for malicious software*
- *The developer cannot be verified*

**Cause:** the GitHub DMG is **ad-hoc signed / not notarized**. Downloads get a `com.apple.quarantine` flag; Gatekeeper then blocks launch and often mislabels it as “damaged.” This is **not** a bad zip and **not** a broken Apple ID on your Mac.

#### Workarounds (recommended)

Try in order:

| # | Method | What to do |
|---|--------|------------|
| 1 | **Clear quarantine** (most reliable) | After install to Applications, run the commands below |
| 2 | **Right-click → Open** | Finder → right-click **Codex Spur** → **Open** → **Open** (do not double-click the first time) |
| 3 | **Privacy & Security** | **System Settings → Privacy & Security** → scroll to the block message → **Open Anyway** |
| 4 | **Build from source** | Local `npm run bundle:dmg` builds usually have **no** browser quarantine flag |

**Method 1 — Terminal (copy-paste):**

```bash
# After dragging Codex Spur into Applications:
xattr -cr "/Applications/Codex Spur.app"
open "/Applications/Codex Spur.app"
```

Only remove the quarantine flag (equivalent intent):

```bash
xattr -d com.apple.quarantine "/Applications/Codex Spur.app" 2>/dev/null
open "/Applications/Codex Spur.app"
```

If the app is still on the Desktop / Downloads instead of Applications, use that path instead of `/Applications/...`.

#### What this is *not*

| Approach | Notes |
|----------|--------|
| Re-downloading the DMG forever | Same quarantine every time from the browser |
| Free Apple ID “signing” | Cannot produce a **Developer ID + notarized** public download |
| `sudo spctl --master-disable` | Turns Gatekeeper off system-wide — **do not** do this for Spur |
| Cracking / faking notarization | Illegal and useless long-term |

#### Permanent fix (maintainers / “double-click just works”)

For strangers to open the DMG with no `xattr` step you need:

1. [Apple Developer Program](https://developer.apple.com/programs/) (~$99/year)
2. **Developer ID Application** certificate in Keychain
3. Tauri **code sign + notarize + staple** before `gh release create`  
   (see [Tauri macOS code signing](https://v2.tauri.app/distribute/sign/macos/))

| Goal | Free / unsigned Release | Developer ID + notarized |
|------|-------------------------|---------------------------|
| Run after you built locally | Usually fine | Fine |
| Open GitHub download with one double-click | Need `xattr` / right-click Open | Yes |
| Silence Gatekeeper for everyone | No reliable free path | Yes |

### Uninstall

1. Quit Codex Spur from the menu bar (not only close the window)  
2. Delete `/Applications/Codex Spur.app`  
3. Optionally delete local data under Application Support  
4. Restore a Codex config backup from the app, or inspect `~/.codex/config.toml` and `~/.codex/codex-select/`

---

## Quick start

1. **Add a provider** — Overview → Add → choose kind & entry method → save & fetch  
2. **Enable models** — Models page → enable routes you want in Codex  
3. **Review & Apply** — preview the diff, then write:  
   - `[model_providers.codex_select]` in `~/.codex/config.toml`  
   - catalog at `~/.codex/codex-select/model-catalog.json`  
4. **Open the Codex model picker** — select any Spur-published model with one click  
5. **Keep Spur running** — the localhost proxy must be up for traffic to reach upstreams  

### Desktop visibility

| Login | Where | Role |
|------|------|------|
| ChatGPT Desktop official login | `~/.codex/auth.json` | GUI identity gate for showing third-party models |
| Spur provider credentials | Spur local vault | Upstream auth for the proxy only — **not** a Desktop login substitute |

Apply may hard-stop if the catalog has non-official slugs and Desktop auth is missing (no fake tokens are written).

### Optional CLI publish

```bash
cargo run --manifest-path src-tauri/Cargo.toml --bin codex-spur-publish
```

Rebuilds catalog from Spur SQLite and publishes into `~/.codex` (for scripts / debugging).

---

## Build from source

### Dependencies

- Node.js 20+ (22 recommended)
- Rust stable (`rustc` ≥ 1.86 per `Cargo.toml`)
- Xcode CLT / standard macOS native toolchain (macOS builds)
- Windows: Visual Studio Build Tools (C++), WebView2 runtime, Rust MSVC target (Windows builds / CI)
- [Tauri 2 prerequisites](https://v2.tauri.app/start/prerequisites/)

### Develop

```bash
npm install
npm run dev:app
```

### Checks

```bash
npm run typecheck
npm run lint
npm run test
cargo fmt --check --manifest-path src-tauri/Cargo.toml
cargo clippy --manifest-path src-tauri/Cargo.toml --all-targets --all-features -- -D warnings
cargo test --manifest-path src-tauri/Cargo.toml
```

### Bundle

```bash
npm run bundle:dmg      # macOS DMG
npm run bundle:nsis     # Windows NSIS (Windows host or CI)
# → src-tauri/target/release/bundle/dmg/Codex Spur_<version>_aarch64.dmg
```

Real-provider smoke tests and reset-credit tests consume quota—**opt-in only**.

---

## Architecture (contributors)

| Layer | Responsibility |
|------|----------------|
| React UI | Presentation & typed Tauri commands only |
| Rust core | Credentials, scheduler, proxy, catalog, Codex config, backups |
| Proxy | Responses-compatible; cancellable; releases leases on disconnect |
| Codex integration | Provider id `codex_select` only; `toml_edit` preserves unrelated config |

Also see:

- [`AGENTS.md`](./AGENTS.md) — engineering & security contract  
- [`DESIGN.md`](./DESIGN.md) — desktop design system  
- [`IMPLEMENTATION.md`](./IMPLEMENTATION.md) — implementation notes  
- [`THIRD_PARTY_NOTICES.md`](./THIRD_PARTY_NOTICES.md) — reference projects  
- [`CHANGELOG.md`](./CHANGELOG.md)  

---

## Paths

| Path | Purpose |
|------|---------|
| `~/Library/Application Support/com.codexspur.desktop/` (macOS) | Local DB, master key, proxy bearer |
| `%APPDATA%\com.codexspur.desktop\` (Windows) | Local DB, master key, proxy bearer |
| `~/.codex/config.toml` / `%USERPROFILE%\.codex\config.toml` | Codex config (backed up before Apply) |
| `~/.codex/codex-select/model-catalog.json` | Published model catalog |
| `~/.codex/auth.json` | Native Desktop login (Spur does not rewrite it in normal operation) |

---

## License & compliance

- MIT — see [`LICENSE`](./LICENSE)  
- Sub2API (LGPL-3.0) is a **behavioral** reference only; source is not copied  
- Codex++ (AGPL-3.0) is architecture reference only  
- Details: [`THIRD_PARTY_NOTICES.md`](./THIRD_PARTY_NOTICES.md)  

Comply with upstream terms and local law. This tool does **not** help bypass CAPTCHAs, phone verification, plan entitlements, or abuse controls.

---

## Disclaimer

Codex Spur is a local integration helper. Upstream APIs and Desktop behavior can change. You are responsible for quota use, account policy, and backups—especially irreversible reset-credit actions.

---

## Maintainers: release checklist

```bash
npm run bundle:dmg
git tag -a v0.1.5 -m "v0.1.5"
git push origin main --tags
gh release create v0.1.5 \
  "src-tauri/target/release/bundle/dmg/Codex Spur_0.1.5_aarch64.dmg" \
  --title "Codex Spur 0.1.5" \
  --notes-file CHANGELOG.md
```

### Optional: Developer ID sign + notarize (removes “app is damaged”)

1. Enroll in [Apple Developer Program](https://developer.apple.com/programs/) and create a **Developer ID Application** certificate in Keychain.
2. Export an App Store Connect API key (or use Apple ID + app-specific password) for notarization.
3. Configure Tauri signing / notarization env vars (see [Tauri macOS code signing](https://v2.tauri.app/distribute/sign/macos/)), then rebuild the DMG and staple the ticket before `gh release create`.
4. For Windows, run the **Windows NSIS release** workflow (`.github/workflows/windows-release.yml`) on `windows-latest`, or build with `npm run bundle:nsis` on a Windows machine. Attach the `*-setup.exe` next to the macOS DMG on the same GitHub Release. v1 Windows installers are **unsigned** (SmartScreen may warn).
