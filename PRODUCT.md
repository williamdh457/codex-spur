# Product

<!-- impeccable:product-schema 1 -->

## Platform

web

## Users

Primary user: a **solo Codex power user** — an individual developer who works daily inside **ChatGPT Desktop / Codex App (GUI)**, not a pure CLI-only workflow.

Situation: they already configure multiple model providers (OpenAI multi-account, Kimi, DeepSeek, xAI/Grok, custom gateways) and need those models available while coding, without leaving Codex, reopening config files, or remembering separate API entrypoints.

Job: **get the right model into Codex’s native model picker (and optionally expose the same stack as a local Responses API) with credentials that never leave the machine.**

## Product Purpose

**Codex Spur** is a local-first desktop control surface that publishes user-selected models into Codex’s existing model picker and can also reverse-proxy those models as OpenAI **Responses** for third-party clients.

It exists so multi-provider model choice becomes a one-click act inside the official Codex UI after a deliberate **Review & Apply**, instead of injection into `ChatGPT.app` or cloud-hosted credential middlemen.

Success means:

1. Selected models appear and switch correctly in Codex’s native picker.
2. Secrets stay local; the UI never receives raw tokens or keys.
3. Closing the main window keeps the proxy alive in the menu-bar process; quitting stops the proxy and releases leases.
4. Codex publish and API 反代 remain independent scopes with clear operator control.

## Positioning

Neighboring switchers (Nice Switch, CC Switch, generic proxies) exist. Spur’s durable claim is the **supported three-seam integration without client injection**:

1. localhost OpenAI Responses–compatible proxy;
2. generated Codex `model_catalog_json`;
3. dedicated provider id `codex_select` (never overwrite unrelated provider tables).

Plus: **local-only encrypted credential store**, opaque stable route slugs (no account/email/secret fingerprints in model ids), and honest reasoning-level mapping across the eight Codex effort rungs.

## Operating Context

- **Shell:** Tauri 2 desktop app (React + TypeScript frontend, Rust core). Ships **macOS (Apple Silicon)** and **Windows x64**; product design optimizes for dense desktop utility use.
- **Primary runtime consumer:** Codex App / ChatGPT Desktop GUI (`model_provider = "codex_select"`, localhost proxy, catalog slugs). Session truth often lives in Codex rollouts (`~/.codex/sessions/**`, `state_5.sqlite`), not only terminal logs.
- **Operator rituals:** add provider instance → fetch models → Models page enable (Codex and/or 反代) → Overview **Review & Apply** → fully quit and reopen Codex when required.
- **Models page scopes:** two peer destinations — **Codex** (catalog publish into official picker) and **反代** (Responses transit). Selection state is independent per scope.
- **Per-model publish metadata:** each route may override **display name** and **context window** (defaults = official heuristics / `供应商 · 模型`). One override covers both Codex catalog and Z Code projection; disk write still requires **Review & Apply**. Z Code only receives `relay_enabled` routes.
- **Menu bar / tray:** proxy status and keep-alive while main window is closed.
- **Codex paths:** macOS `~/.codex`; Windows `%USERPROFILE%\.codex`. App data: macOS `~/Library/Application Support/com.codexspur.desktop/`; Windows `%APPDATA%\com.codexspur.desktop\`.
- **Secondary surface:** marketing/docs site under `website/` and bilingual READMEs — not the operator’s primary work surface.

## Capabilities and Constraints

**Capabilities (confirmed):**

- Unlimited provider instances of the same kind; primary object is **provider instance**, not “account pool” as a peer nav destination.
- OpenAI entry: official OAuth / API key / provider config JSON / multi-account credentials JSON.
- Model discovery as candidates; only enabled routes enter catalog / relay.
- Pool vs Fixed routing for multi-account OpenAI; affinity and load-aware selection with lease release on cancel/error/complete.
- Eight-level reasoning map per route (`none` … `ultra`) with truthful clamps.
- OpenAI quota windows by `limit_window_seconds` (5h / 7d); reset-credit with confirmation, idempotency, audit.
- API 反代: separate process surface; defaults to on at app launch (`relay.desired_running`, explicit Stop remembers off); client keys; model enablement on Models page; optional LAN bind.
- Apply flow: preview/diff, advisory lock, backup, atomic write, journal/recover; never empty TOML fallback on parse failure.

**Hard constraints (product law):**

- Frontend must never receive raw access/refresh tokens, API keys, session cookies, proxy bearer tokens, or decrypted credential payloads.
- Secrets local-only; no telemetry of credentials; redact secrets from logs, UI errors, fixtures, and screenshots.
- Master key in local `0600` `master_key.hex` (not Keychain) for rebuild-friendly identity.
- Bind proxy to `127.0.0.1` by default in v1; do not install LaunchAgent, privileged helper, or unrelated daemons.
- Do not inject or modify `ChatGPT.app` binary; do not casually rewrite native `auth.json` (explicit backed-up sync only).
- Cross-provider same-thread history is unsafe; product/proxy must sanitize non-portable ids and encrypted reasoning — do not treat mid-thread provider switches as always safe.
- Do not bypass CAPTCHAs, plan entitlements, phone verification, or provider abuse controls.
- Sub2API is behavioral reference only (LGPL); do not copy its source. Codex++ is architecture reference only (AGPL). Keep `THIRD_PARTY_NOTICES.md` current when adapting MIT/Apache material.

**Terminology:**

| Term | Meaning |
|---|---|
| Codex scope | Publish route into Codex native model picker via catalog + `codex_select` |
| 反代 / API relay | Expose route as local OpenAI Responses transit for third-party clients |
| Provider instance | User-facing configured provider row (CC Switch–style) |
| Review & Apply | Explicit, previewed write of Codex config + catalog; also projects relay-enabled models into Z Code |
| spur-route / route slug | Stable opaque model id shown to Codex / clients |
| Display / context override | Optional per-route published label and context tokens; null restores official defaults |

**Open / not claimed:** commercial SLAs, multi-user cloud sync, signed notarization as always-on default for public DMG (releases may be ad-hoc / unnotarized — operational fact for install UX, not a design aesthetic).

## Brand Commitments

- **Name:** Codex Spur (app / product). Bundle id family `com.codexspur.desktop`.
- **Tagline (confirmed product copy):** 你配好的模型，全部进 Codex 选择器。一键切换。 / “Your configured models, all in the Codex picker. One-click switch.”
- **Voice:** operational, concrete, safety-explicit; action verbs (`Fetch models`, `Review & Apply`); never blame the user; never surface raw upstream secrets in errors.
- **Assets on hand:** `src/assets/codex-spur-icon.png`, model-picker proof shot `docs/images/codex-model-picker.png` (and `src/assets/codex-model-picker.png`).
- **Visual system authority for UI work:** existing `DESIGN.md` + implementation (dense desktop utility). `DESIGN-cohere.md` is a read-only token/material reference only — not IA to copy.
- In-app copy today is largely Chinese; bilingual READMEs exist. No separate marketing brand book beyond repo docs.

## Evidence on Hand

- Engineering contract: `AGENTS.md`
- Design baseline: `DESIGN.md` (desktop utility; updated Models scope switcher)
- Product narrative & install: `README.md`, `README.zh-CN.md`
- License: MIT (`LICENSE`); third-party notices: `THIRD_PARTY_NOTICES.md`
- Proof imagery: Codex picker screenshot with Spur-published models
- Marketing site source: `website/`
- **Do not fabricate:** testimonials, customer logos, performance benchmarks, paid plan pricing, or notarization claims beyond what releases actually ship.

## Product Principles

1. **Picker is the product** — success is models usable in Codex’s native UI (and optional local Responses), not a second chat surface.
2. **Local secrets, explicit apply** — credentials never leave the machine; config mutation is previewed, locked, backed up, and reversible.
3. **No injection, supported seams only** — proxy + catalog + `codex_select`; never rewrite ChatGPT.app.
4. **Tell the truth about capability** — reasoning clamps, quota state, cross-provider history hazards, and apply restart requirements stay explicit.
5. **Dense operator craft** — calm all-day desktop density; safety and understanding beat decorative chrome.

## Accessibility & Inclusion

No product-specific legal standard was contracted beyond engineering baseline in `DESIGN.md` / `AGENTS.md`: full keyboard access to nav, tables, sheets, and segmented controls; visible focus; WCAG AA contrast targets; status not encoded by color alone; reduced-motion respected for non-essential motion. Desktop minimum usable window remains **900×640**. No confirmed low-vision or motor-impairment research beyond that baseline.
