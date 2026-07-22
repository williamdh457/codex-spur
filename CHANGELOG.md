# Changelog

All notable changes to Codex Spur are documented in this file.

## [Unreleased]

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
