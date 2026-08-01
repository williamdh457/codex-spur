# Changelog

All notable changes to Codex Spur are documented in this file.

## [Unreleased]

## [0.1.13] - 2026-08-01

### Features

- **自定义供应商上游 API 格式**: 添加 / 编辑 **custom**（Base URL + API Key）时，除推理映射模板外可选 **Responses**（`/v1/responses`）或 **Chat Completions**（`/v1/chat/completions`，默认）。写入 `providers.protocol`；代理与 catalog freeform 策略随实例协议刷新，不再只用 kind 默认值。
- **API 反代 Key：wire + 命名**: 反代 client key 支持 `wire_type`（responses / completions）、`name_style`（flat / dotted）与 `allowed_providers`；`POST /v1/responses` 与 `/v1/chat/completions` 按 key 与上游能力校验。迁移 `0011_relay_wire_and_naming.sql`。
- **API 反代中转站（Responses 外放）**: Models 页为每个模型增加独立的 **Codex / 反代** 双开关；反代与 Codex catalog 互不绑定。同进程第二监听器（默认 `127.0.0.1:17862`）提供 `GET /healthz`、`GET /v1/models`、`POST /v1/responses`，鉴权用多把 client API Key（每把可设模型白名单）。可选局域网 bind（`0.0.0.0`）并展示 LAN Base URL。转发核复用现有三车道，不新做 tool 工程；不改 Codex `config.toml`。

### Fixed

- **Three-lane upstream routing (OpenAI / Responses-native / Chat bridge)**: Codex always hits Spur `/v1/responses`. **(1) OpenAI official** — OpenAI product Responses. **(2) Responses-native (DeepSeek-style)** — DeepSeek V4 Flash/Pro and **xAI Grok** (`api.x.ai` preferred API is Responses; Chat Completions is legacy). **(3) Chat Completions bridge (CC Switch-style)** — Kimi, MiniMax, OpenCode Go, custom OpenAI-compatible by default, legacy DeepSeek chat ids. Explicit `Responses` on custom stays native.

### Packaging

- macOS Apple Silicon build for **0.1.13**.
- **Grok/third-party apply_patch dialect (Desktop verification loop)**: Codex Desktop requires the freeform body first line to be exactly `*** Begin Patch` (no trailing stars). Grok and some bridges emit `*** Begin Patch ***`, path glued as `file.ts***`, or invented `*** End of File ***`, causing endless `apply_patch verification failed` retries (session 019fb8d7 ×44). Spur now normalizes apply_patch freeform input on the inbound tool-roundtrip path (JSON + SSE + already-shaped custom_tool_call) and tightens the portable tool description to the Desktop dialect. CC Switch routes Grok via Responses but does not rewrite patch text — this is a Spur multi-vendor fix.

### Features

- **Overview session policy (mid-thread switch)**: main UI segmented control — **允许中途换模型** (default, portable `spur1` compact so OpenAI↔Grok can both read summaries) vs **不中途换模型** with optional **OpenAI 云端加密压缩** (Apply writes `name = "OpenAI"`, proxy passes OpenAI Compact V2). Changing policy requires Review & Apply when cloud compact is involved.
- **DeepSeek V4 native Responses (official Codex path)**: new DeepSeek instances default to **`protocol = Responses`** (no longer Chat Completions). `deepseek-v4-flash` / `deepseek-v4-pro` always forward to upstream **`/responses`** even if a legacy provider row still says Chat — matching DeepSeek’s 2026-07-31 Codex script (`wire_api = "responses"`). Legacy ids such as `deepseek-chat` stay on the Chat Completions bridge. Catalog rows for DeepSeek align with the official script: **1M** context, `effective_context_window_percent = 95`, `truncation_policy.mode = tokens`, `default_reasoning_summary = none`, `apply_patch_tool_type = freeform`, `model_messages.instructions_template` dual-write of the lean `base_instructions`. Still published under Spur’s `codex_select` provider (does **not** monopolize `model_provider = deepseek`).

### Fixed

- **Official context bar / auto-compact alignment**: catalog `effective_context_window_percent` defaults to **95** (Codex official usable UI window). Auto-compact stays **~90% of raw `context_window`** (same clamp as `codex-rs`), i.e. about **~95% of the Desktop context bar**. Legacy Spur rows with 90% are healed on normalize / Apply.
- **Official `apply_patch` for third-party routes (Kimi/DeepSeek/xAI/…)**: catalog rows advertise `apply_patch_tool_type: freeform` so Desktop registers client-side apply_patch required by official lean `base_instructions`. **Do not force `tool_mode=code_mode_only`** (that primary freeform `exec` JS path caused weaker models to thrash on nested `tools.*`); matches Nice Switch GPT-5.5 freeform-only ads and live native gpt-5.6 top-level `exec_command`. Proxy **always injects** a portable Chat/Responses `function` named `apply_patch` for non-OpenAI hosts (even when Desktop omits freeform), rewrites freeform/custom rows, and maps `custom_tool_call` history into Chat tool turns. Kill switch: `SPUR_DISABLE_APPLY_PATCH=1`. `web_search` stays off until a separate A/B; DeepSeek may advertise `supports_parallel_tool_calls` per official catalog.
- **Stop inventing freeform `exec` descriptions**: outbound freeform port uses Desktop description when present; registry fallback for non-`apply_patch` freeform tools is an empty description + `input` schema only (no “legacy freeform exec” product copy).
- **Responses passthrough freeform restore (all providers)**: Grok/xAI and other Responses-native hosts return freeform tools as `function_call` after portable outbound ads — Desktop freeform executor **aborted** (session 019fb6c5). SSE + JSON Responses passthrough now rewrites freeform names (`apply_patch` / `exec`) to official **`custom_tool_call` + freeform `input`** (same gold shape as the Chat Completions bridge). Non-freeform tools unchanged; already-custom items are idempotent. Chat path unchanged.
- **Tool round-trip contract (fix aborted apply_patch)**: Kimi was calling `apply_patch` as Chat `function`, but Spur returned Desktop `function_call` — freeform executor **aborted** with zero output. New `tool_roundtrip` registry restores freeform tools to official **`custom_tool_call` + freeform `input`** (gold sample from native successful sessions) on both stream and non-stream paths. Outbound may adapt; inbound must match Desktop.
- **Full tool registry from local rollouts**: freeform set is exactly **`apply_patch` + `exec`** (custom_tool_call); all other observed tools (`exec_command`, plan/goal, multi-agent, Codex App thread tools, computer-use, common MCP names) are explicit **function_call** profiles. Outbound ports any freeform custom tool (not only apply_patch); unknown names still default to function symmetry.
- **Tool outbound fidelity (official surface)**: when rewriting freeform/custom tools for Chat Completions, **Desktop-supplied name/description/parameters win** over registry stubs. Freeform `exec` is never dropped when listed alongside `exec_command`. No proxy-side tool-catalog nudge or identity rewrite — protocol translation only.

## [0.1.12] - 2026-07-30

### Fixed

- **Official lean `base_instructions` from openai/codex `models.json`**: catalog rows no longer use the 117-char CC Switch stub. **Sol** resolves from the official `gpt-5.6-sol` entry; **Terra/Luna** from their official entries; **all other spur routes** map to the Terra/Luna lean body. Source: `codex-rs/models-manager/models.json` fixtures under `src-tauri/fixtures/official_prompts/`. Tool ads remain lean (no `apply_patch_tool_type`) so Desktop model list stays intact.
- **OpenAI compact is portable (same as Kimi/DeepSeek)**: when Desktop issues Remote Compaction V2, Spur always runs the local summarizer on the **current** model and returns a `spur1:` plaintext envelope — including for OpenAI routes. Everyday OpenAI turns still use Responses as before; only the compact beat is intercepted. This avoids minting `gAAAAA…` ciphertext that other models cannot read after a mid-thread switch. Historical OpenAI ciphertext is still not decryptable; re-`/compact` under the new path to get a portable summary.
- **Compact intercept only on live carrier**: historical `spur1:` / `gAAAAA…` compaction rows alone no longer re-enter the compact shim; only a trailing live `{type:compaction}` control item (no `encrypted_content`) counts as a compact request, so later turns can expand the portable summary and answer normally.

### Packaging

- macOS Apple Silicon build for **0.1.12**.

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
