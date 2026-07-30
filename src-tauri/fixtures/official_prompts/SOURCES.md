# Official Codex lean `base_instructions` fixtures

**Authoritative source (user-approved):**

https://raw.githubusercontent.com/openai/codex/main/codex-rs/models-manager/models.json

Exported from OpenAI `openai/codex` repository field `models[].base_instructions`
for each slug. Also present as `model_messages.instructions_template` for 5.6 rows.

| File | Official slug | Notes |
|------|---------------|--------|
| `gpt-5.6-sol.base_instructions.txt` | `gpt-5.6-sol` | Sol catalog row — load **this** entry for Sol |
| `gpt-5.6-terra.base_instructions.txt` | `gpt-5.6-terra` | Terra + default for non-Sol routes |
| `gpt-5.6-luna.base_instructions.txt` | `gpt-5.6-luna` | Luna (currently byte-identical to terra) |

As of export date 2026-07-30, OpenAI's models.json has **identical** lean
`base_instructions` for sol/terra/luna (17730 chars, Autonomy uses
"Adapt accordingly based on the user's request type"). Sol is still resolved
from the **sol** slug entry so future upstream forks are picked up by re-export;
we never invent a third-party "sol→terra substitute" path.

Refresh:

```bash
curl -fsSL -o /tmp/models.json \
  https://raw.githubusercontent.com/openai/codex/main/codex-rs/models-manager/models.json
# extract base_instructions per slug into the three .txt files
```

Do **not** use asgeirtj leak forks or Desktop 300KB full prompts for catalog rows.
