# Third-party notices

Codex Spur is original software. The following projects were used as **behavioral or architectural references** (not copied as source):

## MIT-licensed references

### CC Switch (cc-switch)

- License: MIT
- Used as a reference for Codex Desktop integration patterns:
  - `supports_websockets = false` on the local Responses provider
  - OpenAI identity gate for Desktop model picker visibility
  - `web_search = "disabled"` ownership sentinel for gateways that reject hosted web search
  - text-only media sanitization approach
  - Chat Completions ↔ Responses adaptation ideas
- Implementation in this repository is independent; no CC Switch source files were copied.

### Nice Switch / related MIT tools

- License: MIT
- Reference for Desktop-native catalog lean rows (`shell_command`, no freeform apply_patch ads) and provider-list UX patterns.

## LGPL-3.0 (not incorporated)

### Sub2API

- License: LGPL-3.0
- **Not** linked or vendored. Scheduler and Grok subscription URL/header behavior are reimplemented from observable contracts and tests only.

## Apache-2.0

### OpenAI Codex

- License: Apache-2.0
- API/schema behavior may be adapted with required notices where applicable.
