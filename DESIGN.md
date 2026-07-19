# Codex Spur Desktop Design System

> Status: implementation baseline  
> Platform: macOS first  
> Updated: 2026-07-19

## 1. Design position

Codex Spur is a dense desktop control surface for providers, models, accounts, routing, quota, and diagnostics. It is not a landing page, a dashboard template, or a visual clone of another switcher.

The interface should feel:

- calm enough to leave open all day;
- compact enough to compare several accounts and models without scrolling through oversized cards;
- explicit enough that a user can understand what will be written to Codex before applying it;
- immediate and physical where interaction benefits from motion;
- restrained where the user is reading tables, quotas, errors, or configuration diffs.

Four priorities guide every decision:

1. **Safety** — credentials and irreversible actions are never ambiguous.
2. **Understanding** — routing, reasoning clamps, quota state, and apply state are explainable.
3. **Speed** — common operations take one or two clicks and provide immediate feedback.
4. **Craft** — spacing, typography, focus, and motion feel native rather than web-template-like.

## 2. Dialectical use of `DESIGN-cohere.md`

`DESIGN-cohere.md` remains a useful visual reference, but its original system was extracted from a marketing and editorial website. Its value is mainly in material, color restraint, and spacing discipline—not information architecture.

### Keep

- Stark white and mineral-neutral surfaces.
- Hairline borders rather than heavy card shadows.
- Near-black primary text with restrained accent color.
- Moderate 8–16px radii for contained surfaces.
- A controlled visual hierarchy with generous whitespace around important decisions.
- Monospaced-feeling labels for technical identifiers and route metadata.

### Translate for desktop

- Large editorial typography becomes 11–24px utility typography.
- Broad feature bands become compact inspectors and status strips.
- Marketing cards become rows, tables, split panes, and popovers.
- Wide page sections become persistent navigation plus a scrollable work area.
- Mobile-first breakpoints become minimum-window and compact-window behavior.

### Reject

- 96px hero titles and 80px section spacing.
- Photo-led cards, trust-logo strips, blog taxonomy, or decorative dark bands.
- Oversized cards for one-line settings.
- Page-length onboarding that prevents users from comparing providers and models.
- Motion used only to make the interface look “alive.”

The correct outcome is visually related to the restrained Cohere reference while structurally behaving like a native administration utility.

## 3. Apple interaction principles

### Immediate response

- Buttons react on pointer-down, not after the command completes.
- Long-running actions immediately replace their icon or label with an inline progress state.
- No artificial delays, loading skeletons for already-local data, or delayed hover transitions.

### Continuity and interruptibility

- Sheets, popovers, inspectors, and the main-window reveal animate from their current presentation value.
- Users may close, reverse, or reopen a moving surface without waiting for the previous animation to finish.
- Network operations remain separately cancellable; UI animation never locks interaction.

### Springs with restraint

Use a critically damped spring for:

- window-level inspector reveal;
- confirmation sheets;
- menu/popover scale and opacity;
- compact disclosure rows.

Suggested baseline:

```text
response: 0.32s
damping ratio: 1.0
overshoot: none
```

Do not spring-animate:

- quota percentage changes;
- table sorting;
- log streaming;
- status color changes;
- model-list filtering;
- progress bars that represent real measurements.

### Reduced motion

With reduced motion enabled:

- remove scale and spatial travel;
- use short opacity changes no longer than 120ms;
- preserve immediate pressed states and focus indicators;
- never animate quota bars from zero on every refresh.

## 4. Window and navigation

### Main window

```text
Default: 1120 × 760
Minimum: 900 × 640
Comfortable maximum content width: none; use split panes
```

The main structure is:

```text
┌──────────────────────────────────────────────────────────┐
│ Toolbar: title / global status / primary action          │
├──────────────┬───────────────────────────┬───────────────┤
│ Sidebar      │ Main work area            │ Inspector     │
│ 208–224px    │ flexible                  │ 320–380px     │
└──────────────┴───────────────────────────┴───────────────┘
```

The inspector is contextual and may be hidden. It opens for model mapping, account details, diagnostics, and apply diffs. At compact widths, it becomes an overlaid sheet instead of compressing the work area below usability.

### Sidebar

Primary destinations:

1. Overview
2. Providers
3. Models
4. Accounts
5. Diagnostics
6. Settings

Navigation rows are 32px high with 8px horizontal padding. Use specific labels; avoid vague categories such as “Manage.”

The lower sidebar contains:

- proxy status;
- Codex binding status;
- version/build metadata;
- a compact “Open logs” action.

### Menu bar

The menu-bar icon communicates only the highest-severity status:

- normal: proxy running and Codex binding valid;
- warning: proxy running but catalog/config needs reapply;
- error: proxy stopped or configuration invalid.

Menu items:

- Open Codex Spur
- Proxy status
- Restart proxy
- Refresh OpenAI quota
- Restore previous Codex configuration
- Quit

Closing the main window hides it. Only Quit stops the proxy.

## 5. Typography

Use the macOS system font stack. Do not bundle a web-display font for utility UI.

```css
font-family: -apple-system, BlinkMacSystemFont, "SF Pro Text", Inter, sans-serif;
```

Use `ui-monospace`, SFMono-Regular for route slugs, model ids, endpoints, response ids, and diagnostic payload keys.

| Role | Size | Weight | Line height |
|---|---:|---:|---:|
| Window title | 20px | 650 | 26px |
| Section title | 16px | 650 | 22px |
| Card/row title | 13px | 600 | 18px |
| Body | 13px | 400 | 19px |
| Control label | 12px | 500 | 16px |
| Caption | 11px | 400 | 15px |
| Micro/status | 10px | 550 | 13px |
| Monospace data | 11px | 450 | 16px |

Use tabular numerals for quota percentages, latency, timestamps, and account counts.

## 6. Color and material

### Light mode

```text
canvas             #F7F7F5
surface            #FFFFFF
surface-raised     #FBFBFA
surface-muted      #F0EFEB
text-primary       #1D1D1F
text-secondary     #68686F
text-tertiary      #96969E
border             #DCDCDE
border-subtle      #E9E9E7
accent             #356F5D
accent-hover       #2E6252
selection          #E4F1EC
success            #2F7D55
warning            #A66A16
error              #B9473F
info               #3C6FA8
```

### Dark mode

```text
canvas             #161715
surface            #1D1F1C
surface-raised     #232521
surface-muted      #292B27
text-primary       #F2F2EF
text-secondary     #B1B2AC
text-tertiary      #7F817A
border             #383A35
border-subtle      #2E302C
accent             #70A991
selection          #29463B
success            #66B58A
warning            #D5A357
error              #E17A72
info               #78A8D6
```

Use color for status and selection, not decoration. A red error state must include an icon and text; color alone is insufficient.

### Shadows and translucency

- Standard rows/cards: no shadow.
- Popovers: one soft shadow and 1px border.
- Overlaid inspector/sheet: subtle shadow, optional translucent toolbar only where content visibly scrolls beneath it.
- Avoid multiple translucent layers that reduce text contrast.

## 7. Spacing, sizing, and radii

Spacing scale:

```text
2, 4, 6, 8, 12, 16, 20, 24, 32
```

Control heights:

```text
compact icon button  24px
small button         28px
standard field       32px
large primary action 36px
table row             38px
account quota row     52–64px
```

Radii:

```text
small controls  6px
fields          8px
cards           10px
popover/sheet   14px
pill            999px
```

Do not use more than three nesting levels of rounded containers. When rows already live inside a bordered panel, inner content should usually be separated by rules rather than more cards.

## 8. Core screens

### Overview

Use a compact status grid, not large analytics cards.

Top strip:

- Proxy: Running / Stopped
- Codex binding: Applied / Changed / Invalid
- Published models: count
- Healthy accounts: count
- Last apply time

Below it:

- “Needs attention” list for expired credentials, provider fetch failures, or apply drift.
- Recent local routing metrics.
- Primary action: `Review & Apply` only when a draft differs from the active revision.

### Providers

Provider list uses rows with:

- vendor icon or monogram;
- name and region;
- protocol badge;
- credential status;
- last model fetch;
- selected/available model count;
- overflow menu.

Preset cards are allowed only in the Add Provider sheet. They must not remain as oversized cards in the main list.

Provider editor sections:

1. Region and base URL
2. Authentication
3. Protocol and models endpoint
4. Connection test
5. Advanced headers and capability defaults

API keys are write-only. After save, show only “Key stored” and a replacement action.

### Models

Use a selectable table with sticky header:

- enabled checkbox;
- picker display name;
- upstream model id;
- provider;
- protocol;
- context;
- tools/image status;
- reasoning summary;
- validation state.

Toolbar:

- provider filter;
- search;
- selected count;
- Fetch models;
- Review mappings.

Default display name format:

```text
供应商 · 模型
```

The inspector shows route identity, capability sources, account availability, and the full reasoning map.

### Reasoning mapping inspector

Hierarchy:

```text
Provider
  Model
    Mapping table
```

Table columns:

- Codex effort
- Upstream value
- Effective behavior
- Source

Rows with identical effective behavior may be visually grouped, but all eight Codex levels remain individually readable. `always_on`, `clamped`, and `ignored` use warning text rather than relying on a badge alone.

The inspector includes:

- Reset to preset
- Validate mapping
- Save override
- A plain-language explanation of why multiple Codex levels may map to one upstream level

### Accounts

Top segmented control:

```text
Pool | Fixed
```

Pool mode shows a dense account table. Fixed mode shows the same account row design with a selected radio state; do not create an unrelated second visual system.

Account row content:

```text
Identity / plan / state
5h quota bar + used/remaining + reset
7d quota bar + used/remaining + reset
reset-credit count
latency/error summary
row actions
```

#### Compact quota layout

Inspired by the information density of Sub2API, without copying its implementation:

```text
5h  ███████░░░  68% used · 32% left · resets 21:40
7d  ███░░░░░░░  29% used · 71% left · Jul 23 09:12
     ↻ Refresh   Reset cards 2   Use 1 card…
```

- Bars are 4px high and do not animate from zero after refresh.
- Used percentage is primary; remaining percentage is adjacent.
- Stale snapshots display a muted `Updated 8m ago` label.
- Unknown data uses `—`, never `0%`.
- Pool summary reports healthy/limited counts; it does not average percentages across unrelated accounts.

Row actions:

- Test with model…
- Refresh quota
- Use reset card…
- Enable/disable
- Edit scheduling
- Reauthorize
- Export…
- Delete…

### Reset-credit confirmation sheet

This is an important-action sheet, not a small popover.

Show:

- masked account label/email;
- current 5h and 7d used percentages;
- available credit count;
- nearest credit expiry;
- statement that one credit will be consumed and the action may be irreversible.

Actions:

- Cancel
- `Use 1 reset card` as the emphasized destructive/important action

After submission, lock duplicate interaction until the idempotent request resolves. An ambiguous timeout must say that the outcome is unknown and offer a retry using the same request id.

### Diagnostics

Use a split view:

- left: timestamped request list;
- right: selected request explanation.

The explanation shows:

- route slug and display name;
- scheduler layer: previous response, session sticky, or load balance;
- anonymized account fingerprint;
- protocol adapter;
- reasoning map applied;
- first-token and total latency;
- result/error category;
- retry/cooldown decision.

No raw tokens, full account ids, request prompts, tool payload secrets, or upstream authorization headers are displayed.

### Apply review

The Apply sheet is a three-stage review:

1. Models that will appear in Codex
2. Generated provider/catalog summary
3. Configuration diff and backup location

States:

- Validate
- Applying
- Verified
- Restart Codex required
- Failed and restored

The final success screen says `Fully quit and reopen Codex` rather than implying the user must sign out of ChatGPT.

## 9. Components and interaction details

### Buttons

- One primary action per visible surface.
- Icon-only buttons require tooltips and accessible labels.
- Destructive actions never use the same accent styling as routine primary actions.
- Press state: scale to 0.98 for 80–100ms unless reduced motion is enabled.

### Fields

- Labels remain visible above fields; placeholders are examples, not labels.
- Secret fields do not offer permanent “show” state. Reveal only while pressed or for a short explicit interval.
- Validation appears beside the field and in a top-level summary when applying.

### Status badges

Badges are reserved for compact categorical state:

- Healthy
- Expired
- Access only
- Refreshable
- Limited
- Cooling down
- Unverified

Do not use badges for full explanatory sentences.

### Tables

- Sticky headers and stable column widths.
- Row hover is subtle; selection is stronger and keyboard-visible.
- Avoid zebra striping unless a table exceeds six dense numeric columns.
- Bulk operations live in the toolbar and appear only with a selection.

### Empty states

Empty states are operational:

- one sentence describing what is missing;
- one primary action;
- an optional secondary documentation link.

No illustrations are required in v1.

## 10. Accessibility

- Full keyboard access to navigation, tables, popovers, sheets, menus, and segmented controls.
- Focus never disappears when an item is removed or a sheet closes; return it to the invoking control.
- Minimum target size is 24×24px for compact desktop icons and 32px for primary controls.
- Text contrast targets WCAG AA.
- Quota status includes text values and is not encoded by bar color alone.
- Live network status uses polite ARIA announcements; irreversible-action results use assertive announcements.
- Avoid automatically moving focus during background refreshes.

## 11. Responsive desktop behavior

At widths below 1000px:

- hide nonessential table columns behind the inspector;
- reduce sidebar to 184px before considering icon-only mode;
- present inspector as an overlay sheet;
- keep quota values and actions readable without horizontal scrolling.

At minimum width, preserve Providers, Models, and Accounts core workflows. Do not create a phone layout for v1.

## 12. Writing style

- Use concrete action verbs: `Fetch models`, `Refresh quota`, `Apply to Codex`.
- Differentiate Save Draft from Apply to Codex.
- Say `5-hour limit` and `7-day limit`; do not use ambiguous `primary` and `secondary` labels in the UI.
- Explain clamps directly: `Codex low and medium both use DeepSeek high`.
- Error messages contain: what failed, what remains unchanged, and the next safe action.
- Never blame the user or expose raw upstream error bodies.

## 13. Visual acceptance checklist

A screen is not complete until:

- the primary task is obvious within two seconds;
- the main content is usable at 900×640;
- keyboard focus is visible and ordered;
- loading, empty, error, stale, disabled, and success states are designed;
- dark mode retains hierarchy and contrast;
- reduced motion is verified;
- no secret can appear in a screenshot;
- quota and mapping tables remain dense without feeling cramped;
- the UI still looks like one application when switching among Pool, Fixed, provider, model, and diagnostic views.
