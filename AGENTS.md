# AGENTS.md — Codex Spur Engineering Contract

## 1. Product intent

Codex Spur is a macOS-first Tauri desktop application that publishes user-selected models into Codex's existing model picker without modifying or injecting code into `ChatGPT.app`.

The product integrates through three supported seams:

1. a localhost OpenAI Responses-compatible proxy;
2. a generated Codex `model_catalog_json`;
3. a dedicated Codex model provider named `codex_select`.

Closing the main window must keep the proxy alive in the menu-bar process. Quitting the app must stop the proxy and release all account leases. Version 1 must not install a LaunchAgent, privileged helper, or unrelated background daemon.

## 2. Required implementation order

When bootstrapping or rebuilding the repository:

1. create and maintain this `AGENTS.md` first;
2. preserve `DESIGN-cohere.md` as a read-only visual reference;
3. create and maintain the desktop-specific `DESIGN.md`;
4. only then scaffold or edit application source.

Do not delete, move, or reinterpret unknown user files without explicit approval. Existing root-level JavaScript research artifacts are not application entrypoints and must not be included in the production bundle.

## 3. Architecture boundaries

### Frontend

- React + TypeScript is responsible for presentation, interaction state, accessibility, and invoking typed Tauri commands.
- The frontend must never receive raw access tokens, refresh tokens, API keys, session cookies, proxy bearer tokens, or decrypted credential payloads.
- TypeScript types exposed over IPC must be generated from or checked against Rust-authoritative schemas.

### Rust application core

- Rust owns provider configuration, model discovery, credential normalization, encryption, account scheduling, quota operations, protocol adaptation, Codex configuration writes, backups, and the localhost proxy.
- Domain logic must not depend directly on Tauri window types. Put Tauri commands at the boundary so core modules remain testable without a GUI.
- Long-running network operations must be cancellable and must not block the Tauri event loop.

### Local proxy

- Bind only to `127.0.0.1` in v1.
- Require a per-install bearer token for every route except a deliberately minimal health probe.
- Accept Codex-facing OpenAI Responses requests and normalize upstream Responses, Chat Completions, Anthropic Messages, and ChatGPT Codex backend traffic.
- Client disconnects must cancel upstream work and release leases.
- Do not advertise WebSocket, image, search, audio, service-tier, or parallel-tool capabilities unless the route has been verified to support them.

### Codex integration

- Use a dedicated provider id: `codex_select`. Never overwrite an existing `custom`, Nice Switch, CC Switch, or unrelated provider table.
- Preserve comments and unrelated TOML sections with `toml_edit`-style structural edits.
- Generate stable opaque route slugs; do not expose account ids, emails, provider secrets, or credential fingerprints in model slugs.
- Normal operation must not modify Codex's native `auth.json`. Native-account synchronization is a separate, explicit, backed-up action.

## 4. Security and privacy rules

- Secrets are local-only. No telemetry or remote service may receive imported credentials.
- Store the random master key only in a local `0600` file under the app data dir (`master_key.hex`). Do **not** use macOS Keychain for the master key: unsigned/dev rebuilds get a new code identity and Keychain re-prompts the login password on every launch. Store credential payloads in SQLite as AES-256-GCM ciphertext with unique nonces, authenticated metadata, and a credential version.
- Use keyed irreversible fingerprints for deduplication; do not persist raw token hashes that can be correlated across installations.
- Zeroize decrypted secret buffers where practical and keep their lifetime narrow.
- Never place secrets in:
  - logs or tracing fields;
  - panic messages or crash reports;
  - UI errors or clipboard helpers;
  - fixtures, snapshots, screenshots, or example config;
  - command-line arguments visible to other processes.
- Redact bearer tokens, API keys, cookies, email addresses, account ids, JWTs, authorization headers, and upstream response bodies before logging.
- Imported ChatGPT Web sessions without a real refresh token are `access_only`; they must not be presented as refreshable.
- Synthetic or placeholder ID tokens are allowed only in an explicit compatibility export. They are never trusted for internal authentication.
- Reset-credit consumption is an important, irreversible action: require an explicit confirmation, use an idempotency key, and never retry with a new key after an ambiguous timeout.
- The product must not bypass account restrictions, CAPTCHAs, phone verification, plan entitlements, or provider abuse controls.

## 5. Provider instances and scheduling invariants

The primary user-facing object is a **provider instance** (CC Switch–style), not an account-pool product.

- Users may add unlimited instances of the same kind (several OpenAI, several Kimi, several DeepSeek, …).
- Adding is the primary action: choose kind + entry method → save and fetch models → a new row appears on the Overview provider list.
- Entry methods for OpenAI include: API/official form, import provider config JSON, and import multi-account credentials JSON.
- “Account pool” is an internal runtime construct (default pool per instance) for multi-credential scheduling. It must not be a co-equal primary UI surface next to API/JSON configuration.

Within a multi-account OpenAI instance, routing has exactly two modes:

```text
Pool { pool_id }
Fixed { account_id }
```

Pool scheduling order is:

1. `previous_response_id` affinity;
2. session-hash affinity;
3. filtered, load-aware Top-K weighted selection.

Every selected account must pass provider, model, capability, token validity, cooldown, quota, and concurrency checks. Sticky bindings must escape and rebind when the account is unhealthy or unusable. Leases must expire after crashes and be released on success, error, cancellation, or stream termination.

The Sub2API scheduler is a behavioral reference only. Reproduce observable behavior with independent code and parity tests; do not translate or copy its LGPL implementation.

## 6. Reasoning mapping invariants

The Codex-facing ladder is always:

```text
none, minimal, low, medium, high, xhigh, max, ultra
```

Each model route must contain an explicit row for all eight levels. A row records the upstream patch, effective native behavior, source, and explanation. If an upstream model cannot disable or vary reasoning, say so truthfully; do not pretend that distinct Codex levels produce distinct upstream behavior.

Reasoning patches may only modify approved reasoning fields. They must never alter model selection, input/messages, tools, stream flags, authentication, or arbitrary headers.

## 7. Quota and reset-credit invariants

OpenAI account views show the nearest 5-hour and 7-day windows by `limit_window_seconds`, not by assuming primary/secondary ordering. Display used percentage, remaining percentage, reset time, fetched time, and staleness.

Quota refresh failures must not automatically disable an otherwise healthy account. Explicit authentication failures, expired access-only credentials, refresh failures, model incompatibility, and rate limits must remain distinguishable.

A reset-credit action applies to exactly one real account. Persist its request id and audit result, disable duplicate submission, refresh quota and credit counts after success, and fail closed when account identity is ambiguous.

## 8. File mutation and recovery rules

Before changing Codex configuration:

1. read and parse all target files;
2. calculate and show a preview/diff;
3. record content hashes;
4. acquire an advisory lock;
5. abort if files changed after preview;
6. create an encrypted or permission-restricted backup;
7. write to a sibling temporary file;
8. flush and fsync;
9. atomically rename;
10. read back and parse;
11. roll back on any failure.

Maintain an apply journal so startup can detect and recover an interrupted multi-file update. Never fall back to an empty TOML document when parsing fails.

## 9. License boundaries

Reference repositories have different licenses and must not be treated as interchangeable:

- OpenAI Codex: Apache-2.0 — API/schema behavior may be adapted with required notices.
- Nice Switch, CC Switch, Codex Tools, and GPTSession2CPAandSub2API: MIT — permitted reference/adaptation requires attribution and preservation of notices where code is reused.
- Sub2API: LGPL-3.0 — do not copy source into this repository; implement scheduler and quota behavior independently from observable behavior and tests.
- Codex++: AGPL-3.0 — architecture reference only; do not copy source or create an AGPL-derived implementation unless the project license is deliberately changed and approved.

Keep `THIRD_PARTY_NOTICES.md` current whenever code or substantial implementation material is adapted.

## 10. Design rules

`DESIGN.md` is authoritative for the application UI. `DESIGN-cohere.md` is only a visual-token reference.

- Build a dense desktop utility, not a marketing site.
- Immediate press feedback, keyboard navigation, visible focus, dark mode, and reduced motion are required.
- Gesture or sheet motion must be interruptible and start from the current presentation value.
- Avoid decorative animation in tables, quota bars, logs, and diagnostics.
- Important/destructive actions use clear confirmation sheets and unambiguous labels.

## 11. Verification contract

Before claiming a change is complete, run the narrowest applicable checks and report what was not run.

Expected command families after scaffolding:

```bash
npm run typecheck
npm run lint
npm run test
npm run build
cargo fmt --check --manifest-path src-tauri/Cargo.toml
cargo clippy --manifest-path src-tauri/Cargo.toml --all-targets --all-features -- -D warnings
cargo test --manifest-path src-tauri/Cargo.toml
```

Additional required coverage:

- credential import, encryption, redaction, and expiry;
- all eight reasoning rows and provider clamps;
- Responses/Chat/Anthropic text, SSE, tool calls, errors, and cancellation;
- affinity, weighted selection, leases, cooldown, quota filtering, and sticky escape;
- 5-hour/7-day parsing and reset-credit idempotency;
- Codex catalog schema, TOML preservation, atomic apply, crash recovery, and restore;
- secret scanning of logs, fixtures, snapshots, and example files.

Real-provider smoke tests must be opt-in because they may consume quota. Never run a reset-credit test against a real account automatically.

## 12. Change discipline

- Keep changes scoped to the requested capability.
- Prefer a simple vertical slice over speculative abstractions.
- Do not reformat or rename unrelated code.
- State assumptions that materially affect behavior.
- When an upstream API is undocumented or unstable, isolate it behind a small adapter and fail gracefully rather than spreading assumptions through the codebase.
