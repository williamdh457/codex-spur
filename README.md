<p align="center">
  <img src="src/assets/codex-spur-icon.png" alt="Codex Spur" width="128" height="128">
</p>

<h1 align="center">Codex Spur</h1>

<p align="center">
  <b>English</b> · <a href="./README.zh-CN.md">中文</a>
</p>

<p align="center">
  <strong>Local-first</strong> model &amp; account router for OpenAI Codex / ChatGPT Desktop on macOS.
</p>

<p align="center">
  <a href="https://github.com/williamdh457/codex-spur/releases/latest">Download DMG</a>
  ·
  <a href="./CHANGELOG.md">Changelog</a>
  ·
  <a href="./LICENSE">MIT License</a>
</p>

---

## About

Codex Spur is a **local-first** control surface for the models you actually use—not a cloud account locker and not a patcher for `ChatGPT.app`.

**Privacy by design.** API keys, session tokens, refresh tokens, and proxy bearer secrets stay on this Mac. They are encrypted at rest, never shown to the React UI, and never uploaded to a Codex Spur service. There is no telemetry channel for credentials.

**Codex-native switching.** After you enable models and **Review & Apply**, they appear in the Codex / ChatGPT Desktop model picker. From that picker you can **one-click switch** among every model you configured—OpenAI, Kimi, DeepSeek, xAI, custom gateways, multi-account pools—using the same UI you already use for official models.

**No app injection.** Spur integrates only through supported seams:

1. a localhost OpenAI Responses–compatible proxy  
2. a generated `model_catalog_json`  
3. a dedicated provider id: `codex_select`  

Closing the main window keeps the menu-bar proxy alive. Quitting the app stops the proxy and releases account leases. Version 1 does **not** install a LaunchAgent or privileged helper.

| | |
|---|---|
| Platform | macOS (Apple Silicon first) |
| Stack | Tauri 2 · React · TypeScript · Rust |
| Version | **0.1.0** |
| License | MIT |

---

## Features

### Provider instances

- Add unlimited instances of the same kind (several OpenAI, several Kimi, …)
- **Add → save & fetch models → a new row on Overview**
- OpenAI entry methods: official browser OAuth (PKCE), API key, multi-account credentials JSON, provider config JSON
- Kimi Code defaults to `https://api.kimi.com/coding/v1`
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

- macOS **Apple Silicon** (`aarch64` DMG for this release)
- ChatGPT Desktop / Codex installed (third-party rows in the GUI usually need a valid Desktop login—see Desktop visibility)
- Network access to the upstream APIs you configure

### From Release

1. Open the [latest Release](https://github.com/williamdh457/codex-spur/releases/latest) and download  
   `Codex.Spur_0.1.0_aarch64.dmg` (GitHub may normalize spaces in the asset name)
2. Open the DMG and drag **Codex Spur** into Applications
3. If Gatekeeper blocks first launch: **System Settings → Privacy & Security → Open Anyway** (or right-click → Open)
4. Leave the menu-bar process running while you use Spur-backed models

> Builds are commonly unsigned / un-notarized for personal distribution. Sign and notarize yourself for enterprise deployment.

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
- Xcode CLT / standard macOS native toolchain
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

### Bundle DMG

```bash
npm run bundle:dmg
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
| `~/Library/Application Support/com.codexspur.desktop/` | Local DB, master key, proxy bearer |
| `~/.codex/config.toml` | Codex config (backed up before Apply) |
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
git tag -a v0.1.0 -m "v0.1.0"
git push origin main --tags
gh release create v0.1.0 \
  "src-tauri/target/release/bundle/dmg/Codex Spur_0.1.0_aarch64.dmg" \
  --title "Codex Spur 0.1.0" \
  --notes-file CHANGELOG.md
```
