# Changelog

All notable changes to Codex Spur are documented in this file.

## [Unreleased]

## [0.1.11] - 2026-07-30

### Fixed

- **CC Switch catalog identity (revert 0.1.9 long-prompt mapping)**: multi-route `base_instructions` again use the **117-char neutral** identity from CC Switch’s native template — not `model_instructions_file`, not full `models_cache` GPT agent bodies (~17KB), and not mis-mapped unrestricted files. Desktop still assembles Skills / Plan / Goal / MCP / AGENTS at request time.
- **Heal polluted catalog rows**: empty, `[MODE: UNRESTRICTED]…`, and mis-mapped GPT long agent texts are rewritten to the CC Switch neutral identity on every normalize / Apply.
- **Chat bridge `collapse_system_messages_to_head`**: after `developer` → `system` mapping, all system messages are merged into `messages[0]` (CC Switch / MiniMax-compatible). Plan/Goal developer blocks no longer sit mid-conversation as extra system roles.

### Packaging

- macOS Apple Silicon DMG for **0.1.11** (ad-hoc signed, not notarized — see Gatekeeper / build-from-source notes in the Release).

## [0.1.10] - 2026-07-29

### Fixed

- **Reasoning mapping aligned to real vendor ladders**: each provider instance has a user-selected **reasoning profile template** (OpenAI native, OpenAI-compat conservative, DeepSeek, Kimi, xAI, MiniMax, GLM, Qwen, passthrough). Catalog levels are honest subsets; proxy maps Codex `none…ultra` to real upstream tokens (`thinking`, `reasoning_effort`, `enable_thinking`, …).
- **DeepSeek V4**: maps to thinking on/off + `high`/`max` (xhigh → max, not medium).
- **OpenAI-compat gateways**: clamp xhigh/max/ultra → `high` so unknown efforts do not silently fall back to medium.
- **OpenAI native / GPT custom**: identity mapping; catalog levels prefer `models_cache.json` when present.
- **Apply preserves `model_reasoning_effort`** when still legal for the selected model instead of always rewriting medium.

### Packaging

- macOS Apple Silicon DMG for **0.1.10**.

## [0.1.9] - 2026-07-29

### Fixed

- **Official prompt mapping (no more stub system prompt)**: catalog `base_instructions` are mapped live from Codex home — `model_instructions_file` → inline `base_instructions` → `models_cache.json` — instead of the historical 117-char “You are Codex…” stub that replaced Desktop’s full agent system prompt (~17KB). Edit your official/override prompt and re-Apply (or restart Spur) to rematerialize every route.
- **Compact prompt mapping**: local compact shim prefers `experimental_compact_prompt_file` / `compact_prompt` from `config.toml`; otherwise uses the official OSS compact template (not a Spur-authored handoff).
- **Plan / Goal passthrough**: Spur does not author plan/goal templates; request `developer` bodies are role-mapped for Chat bridges without rewriting content.
- **Stub pollution heal**: empty or short stub `base_instructions` are cleared or overwritten on every catalog normalize; refuse to re-publish the old stub as “official”.

### Packaging

- macOS Apple Silicon DMG for **0.1.9**.

## [0.1.8] - 2026-07-26

### Fixed

- **Custom / 中转站 enable no longer fail-closed on native Compact V2**: third-party Responses and Chat routes can be enabled without an upstream Compact V2 probe. Mid-thread compact is handled by a **local proxy shim** on the **current** model (no cross-vendor handoff / same-vendor matching).
- **Local Remote Compaction V2 shim (`spur1:`)**: for non-OpenAI routes, compact turns summarize on the current model and return exactly one portable `{type:compaction, encrypted_content:"spur1:…"}` item so Codex Desktop does not fatal. Replay decodes `spur1:` (and `ocx1:` interop) into plain text; foreign OpenAI ciphertext becomes an honest opaque note (no fake decrypt).
- **Models page enable switches**: WKWebView-safe `role="switch"` control so toggles are clickable again.

### Packaging

- macOS Apple Silicon DMG for **0.1.8**.
- GitHub Release notes include Gatekeeper “app is damaged” workarounds (`xattr -cr`) and **build-from-source** guidance when the DMG is blocked.

## [0.1.7] - 2026-07-26

### Fixed

- **Remote Compaction V2 compatibility gate**: enabling or publishing a third-party OpenAI-compatible Responses route now probes Compact V2 through the local proxy. Gateways that only accept chat (or return zero/multiple compaction outputs) are disabled with a clear error instead of hard-failing mid-thread in Codex Desktop.
- **Live compaction carrier sanitization**: keep the trailing live `compaction` control item for both OpenAI and non-OpenAI Responses paths; drop historical encrypted compact/reasoning blocks and strip sticky `previous_response_id` after sanitization.
- **Compact response validation**: successful remote-compact responses are buffered and checked for exactly one `compaction` output (JSON or SSE `response.completed`) before they reach Desktop.

### Packaging

- macOS Apple Silicon DMG for **0.1.7**.
- GitHub Release notes include Gatekeeper “app is damaged” workarounds (`xattr -cr`) and **build-from-source** guidance when the DMG is blocked.

## [0.1.6] - 2026-07-24

### Features

- **Settings → 应用更新 / one-click update**: check the latest GitHub Release and download the matching macOS DMG, replace the installed app, and relaunch. No Apple Developer ID / notarization required for the check itself; Gatekeeper workarounds still apply after install.

### Fixed

- Harden context compaction across catalog, proxy, storage, and provider paths (carry-forward from unreleased work on `main`).

### Packaging

- macOS Apple Silicon DMG for **0.1.6**.
- GitHub Release notes include Gatekeeper “app is damaged” workarounds (`xattr -cr`) and build-from-source guidance.

## [0.1.5] - 2026-07-23

### Fixed

- **Session import hard-fail asymmetry**: “导入 session 文件” no longer aborts the whole import when OpenAI `agent/register` returns `agent_registry_not_enabled` (or any registry error). It now uses the same best-effort Agent Identity upgrade + **access-only fallback** as “导入账号 JSON” (Sub2API-style session import behavior). UI copy updated accordingly.
- **Ghost models after provider delete**: deleting a provider now best-effort rewrites `~/.codex/codex-select/model-catalog.json` from remaining enabled routes, so Desktop no longer keeps picker entries (e.g. `723 · GPT-5.6-Sol`) that map to missing credentials (`no_upstream_credential` / 401).

### Packaging

- macOS Apple Silicon DMG for **0.1.5**.
- GitHub Release notes include Gatekeeper “app is damaged” workarounds (`xattr -cr`).

## [0.1.4] - 2026-07-23

### Fixed

- Prevent provider imports from spinning forever after credentials and models were already committed.
- Decouple successful provider creation from the best-effort Overview snapshot refresh so refresh failures cannot roll back imported data.
- Add bounded model-discovery network timeouts and a five-second runtime-refresh timeout.

## [0.1.3] - 2026-07-23

### Features

- Add first-class **OpenCode Go** provider support with secure local credential import, manual API-key fallback, model discovery, and Chat Completions proxy routing.

## [0.1.2] - 2026-07-22

### Features

- **OpenAI entry simplified to three methods only**:
  1. Official ChatGPT browser OAuth (PKCE)
  2. Import account JSON (single/multi)
  3. Import ChatGPT session dump
- Removed OpenAI **API Key** and **provider config JSON** entry paths from the add/edit UI (legacy API-key instances still run).
- **Agent Identity** for ChatGPT sessions: register Ed25519 runtime via `auth.openai.com` `agent/register`, store only runtime + private key, sign upstream requests with `AgentAssertion` (no SMS OAuth path required for session import).
- Session/account import auto-discovers official Codex models so the provider becomes usable immediately after import (new instance or add-to-existing).

### Packaging

- macOS Apple Silicon DMG for **0.1.2**.
- Windows NSIS continues via tag-triggered `windows-release.yml`.

## [0.1.1] - 2026-07-21

### Fixed

- **Cross-provider mid-thread switches** in Codex App / Desktop:
  - OpenAI Responses path drops **all** replayed `reasoning` items (foreign `encrypted_content` and Chat-bridge summary-only reasoning are not portable).
  - Non-OpenAI Responses path (xAI/Grok, MiniMax, custom, …) also drops **all** reasoning and strips `previous_response_id` after sanitization — fixes GPT → Grok `Could not decrypt the provided encrypted_content`.
  - Chat Completions bridge (DeepSeek/Kimi): preserve `function_call` / `function_call_output` history and emit streaming `tool_calls` as Responses function-call items — fixes silent empty turns after Grok/DeepSeek agent work.
- Document bidirectional proxy sanitization invariants in `AGENTS.md`.
- Clarify Gatekeeper “app is damaged” install workaround for unsigned GitHub DMG downloads.

### Packaging

- macOS Apple Silicon DMG for **0.1.1** (still ad-hoc / un-notarized unless you sign with your own Developer ID).

## [0.1.0] - 2026-07-20

### Highlights

- First public macOS release of **Codex Spur**, a local-first model and account router for OpenAI Codex / ChatGPT Desktop.
- Publishes user-selected third-party and multi-account routes into Codex’s model picker **without modifying or injecting into** `ChatGPT.app`.

### Features

- **Provider instances** (CC Switch–style): add unlimited OpenAI / Kimi / DeepSeek / MiniMax / xAI / custom instances.
- **OpenAI entry methods**: official browser OAuth (PKCE), API key, multi-account credentials JSON, provider config JSON.
- **Local Responses proxy** on `127.0.0.1` with per-install bearer token.
- **Codex integration** via dedicated provider id `codex_select` + generated `model_catalog_json`.
- **Reasoning ladder** for every route: `none`, `minimal`, `low`, `medium`, `high`, `xhigh`, `max`, `ultra`.
- **Multi-account scheduling**: Pool / Fixed, sticky affinity, load-aware Top-K weighted selection, leases, cooldowns.
- **Quota views** for OpenAI 5-hour and 7-day windows; optional reset-credit action with confirmation and idempotency.
- **Menu bar residency**: closing the main window keeps the proxy alive; quitting the app stops it and releases leases.
- **Desktop visibility checks** so third-party models can appear in the ChatGPT Desktop picker when conditions are met.
- **Diagnostics**: redacted proxy request events for selection layer, retries, and cooldowns.

### Security

- Secrets stay local. Frontend never receives raw access tokens, refresh tokens, or API keys.
- Credential payloads stored in SQLite as AES-256-GCM ciphertext; master key in a `0600` local file (`master_key.hex`).
- Logs and UI errors are redacted for tokens, emails, and authorization material.

### Packaging

- macOS **DMG** (Apple Silicon / `aarch64`) via Tauri 2.

### Known limitations

- Some streaming / tool-call / Anthropic Messages paths still return explicit “not implemented” errors instead of silent success.
- Official OpenAI catalog advanced tool / visibility fields are not fully mapped yet.
- Real-provider smoke tests and reset-credit tests are opt-in; never run reset-credit against production accounts automatically.
