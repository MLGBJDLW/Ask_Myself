# Provider Settings UX: Primary-source Research

This note supports the AI provider/settings experience requested on 2026-08-08.
It compares current upstream source at immutable revisions and maps the useful
interaction patterns onto Nexa's existing settings primitives. It is a design
input for an independent Nexa implementation, not permission to copy another
project's source, visual design, wording, or assets.

## Executive decision

Nexa should turn AI Providers into a compact status-first settings surface:

1. Keep configured chat/reasoning providers and the add-provider action as the
   primary content.
2. Move the large capability-registry projection behind a collapsed Advanced
   disclosure with a persistent health/count summary.
3. Give each provider category a compact header that remains useful when its
   details are collapsed: configured count, active/default provider, and any
   actionable error.
4. Make provider discovery searchable and responsive, with configured and
   recommended providers first and the full catalog available on demand.
5. Preserve `configured`, `enabled`, `default`, `available`, and `error` as
   separate states. A single green dot or star must not stand for all of them.
6. Reuse Nexa's native-button disclosure primitives, add explicit control/panel
   relationships, preserve form values while collapsed, and validate the page
   at a 320 CSS-pixel equivalent width.

This direction is supported by four distinct primary-source patterns:
Open WebUI's compact connection rows and enable gates, LobeChat's
enabled/disabled grouping and searchable responsive navigation, Dify's compact
credential/status summaries with lazy model-list expansion, and LibreChat's
searchable desktop/mobile settings shell and concise provider-key status rows.
No one upstream should be copied wholesale.

## Scope and reviewed revisions

| Source | Immutable revision | What was inspected |
| --- | --- | --- |
| Open WebUI | [`01f4282f1ffe0d6212f58d3afbeae21fffd0c4be`](https://github.com/open-webui/open-webui/commit/01f4282f1ffe0d6212f58d3afbeae21fffd0c4be) | Connection enable gates, compact connection rows, configuration actions |
| LobeChat | [`ca27228d55bb604f8bcf455a18f5e87d3ba9f9a5`](https://github.com/lobehub/lobe-chat/commit/ca27228d55bb604f8bcf455a18f5e87d3ba9f9a5) | Provider grouping, search, desktop/mobile navigation, provider detail forms |
| Dify | [`d1fa17032eaceba8b2a3fe266a69bfb1e5977aec`](https://github.com/langgenius/dify/commit/d1fa17032eaceba8b2a3fe266a69bfb1e5977aec) | Configured/unconfigured groups, compact status cards, lazy model lists, selector statuses |
| LibreChat | [`45cc53c40b47645b887c3bb996168e06aaa83f4c`](https://github.com/danny-avila/LibreChat/commit/45cc53c40b47645b887c3bb996168e06aaa83f4c) | Responsive settings navigation, settings search, concise provider-key status rows |
| W3C WAI | Current official APG and WCAG 2.2 guidance reviewed 2026-08-08 | Disclosure/accordion keyboard semantics and reflow |

The comparison deliberately covers both formally open-source and
source-available projects. The license section records the distinction.

## Current Nexa seams

Nexa already has most of the implementation vocabulary needed for the upgrade:

- [`ProvidersSettingsTab.tsx`](../../apps/desktop/src/components/settings/ProvidersSettingsTab.tsx#L98-L288)
  places the entire tab inside one non-collapsible `Section`. It renders the
  large `CapabilityRegistryPanel` first, before the add action and configured
  chat providers. The add-provider selector is a fixed two-column grid over the
  complete preset catalog, without search or state grouping.
- The same file already separates chat/reasoning, image generation, and speech
  with `data-provider-category` boundaries. These are the right category seams;
  they need compact summaries and consistent disclosure behavior rather than a
  new persistence model.
- [`SettingsSection.tsx`](../../apps/desktop/src/components/settings/SettingsSection.tsx#L18-L93)
  already supports a `summary`, a native `button`, `aria-expanded`, animated
  disclosure, and reduced-motion behavior. Its smaller
  [`CollapsiblePanel`](../../apps/desktop/src/components/settings/SettingsSection.tsx#L105-L166)
  offers the same interaction for nested panels. Neither currently connects
  the trigger and panel with stable `id`/`aria-controls` metadata.
- [`CapabilityRegistryPanel.tsx`](../../apps/desktop/src/components/settings/CapabilityRegistryPanel.tsx#L333-L510)
  is an expert-oriented projection with connections, models, capability routes,
  permissions ownership, advanced ownership, and mode switching. It is useful,
  but its current first-position expanded layout dominates the page.
- Image, TTS, and STT panels already use internal expansion. A category-level
  redesign must avoid double-disclosure nesting that makes users open two
  headers to reach one form.
- Existing focused coverage in
  [`settings-provider-models.spec.ts`](../../apps/desktop/e2e/settings-provider-models.spec.ts#L829-L1080)
  protects provider/model selection, image and speech settings, credential
  reuse boundaries, and local/cloud behavior. The UX change should extend this
  suite rather than replace its security and capability assertions.

The implementation should remain a projection over Nexa's existing catalog and
backend state. In particular, it must not loosen the exact endpoint/credential
trust boundary or infer credential ownership from a similar-looking URL.

## Primary-source findings

### 1. Open WebUI: compact connections and conditional detail

Open WebUI's administrator Connections form puts OpenAI and Ollama behind
separate enable switches. The connection lists are rendered only when their
family is enabled, and the main body scrolls independently of the persistent
form action. See the pinned
[`Connections.svelte`](https://github.com/open-webui/open-webui/blob/01f4282f1ffe0d6212f58d3afbeae21fffd0c4be/src/lib/components/admin/Settings/Connections.svelte).

Each OpenAI-compatible connection is then projected as a compact row: a
read-only base URL, an explicit configure action, and a separate enable switch.
The disabled row is visually de-emphasized without deleting its saved
configuration. See
[`OpenAIConnection.svelte`](https://github.com/open-webui/open-webui/blob/01f4282f1ffe0d6212f58d3afbeae21fffd0c4be/src/lib/components/admin/Settings/Connections/OpenAIConnection.svelte).
Its section wrapper is intentionally lightweight rather than another large
card, as shown by
[`AdminSettingSection.svelte`](https://github.com/open-webui/open-webui/blob/01f4282f1ffe0d6212f58d3afbeae21fffd0c4be/src/lib/components/admin/Settings/AdminSettingSection.svelte).

**Pattern to use:** keep saved rows visible and compact, separate enablement
from configuration, and reveal editing detail only when requested.

**Boundary for Nexa:** do not invent an enable switch for legacy `AgentConfig`
rows that do not have a backend enable state. Where Settings V2/Connection
records expose `enabled`, show it as an independent control; elsewhere show
only states the backend can truthfully persist.

### 2. LobeChat: state grouping, search, and responsive list/detail navigation

LobeChat's provider overview derives three separate lists and renders counts for
enabled, disabled custom, and disabled built-in providers. That keeps the most
actionable entries first while retaining access to the long tail. See
[`ProviderGrid/index.tsx`](https://github.com/lobehub/lobe-chat/blob/ca27228d55bb604f8bcf455a18f5e87d3ba9f9a5/src/routes/%28main%29/settings/provider/%28list%29/ProviderGrid/index.tsx).
Provider cards place enablement in its own control via
[`EnableSwitch.tsx`](https://github.com/lobehub/lobe-chat/blob/ca27228d55bb604f8bcf455a18f5e87d3ba9f9a5/src/routes/%28main%29/settings/provider/%28list%29/ProviderGrid/EnableSwitch.tsx),
rather than making card selection imply activation.

The provider menu filters by stable ID, display name, or description and has an
explicit empty result in
[`SearchResult.tsx`](https://github.com/lobehub/lobe-chat/blob/ca27228d55bb604f8bcf455a18f5e87d3ba9f9a5/src/routes/%28main%29/settings/provider/ProviderMenu/SearchResult.tsx).
Desktop uses a side-by-side provider menu and detail container
([`Desktop/index.tsx`](https://github.com/lobehub/lobe-chat/blob/ca27228d55bb604f8bcf455a18f5e87d3ba9f9a5/src/routes/%28main%29/settings/provider/_layout/Desktop/index.tsx));
mobile shows either the provider menu or the chosen detail instead of squeezing
both into one viewport
([`Mobile.tsx`](https://github.com/lobehub/lobe-chat/blob/ca27228d55bb604f8bcf455a18f5e87d3ba9f9a5/src/routes/%28main%29/settings/provider/_layout/Mobile.tsx)).
The provider detail form also applies a small-screen full-width rule and groups
configuration under one provider heading in
[`ProviderConfig/index.tsx`](https://github.com/lobehub/lobe-chat/blob/ca27228d55bb604f8bcf455a18f5e87d3ba9f9a5/src/routes/%28main%29/settings/provider/features/ProviderConfig/index.tsx).

**Patterns to use:** group by real state, provide provider search over ID/name/
description, and switch from list-plus-detail to list-or-detail at narrow
widths.

**Pattern not to copy:** LobeChat's overview card uses a clickable `div` for
provider selection in
[`ProviderGrid/Card.tsx`](https://github.com/lobehub/lobe-chat/blob/ca27228d55bb604f8bcf455a18f5e87d3ba9f9a5/src/routes/%28main%29/settings/provider/%28list%29/ProviderGrid/Card.tsx).
Nexa should retain native buttons or links with visible focus treatment.

### 3. Dify: compact truth-bearing status and lazy expansion

Dify's model-provider body separates configured providers from providers still
to be configured, while keeping loading and empty states explicit. See
[`model-provider-page-body.tsx`](https://github.com/langgenius/dify/blob/d1fa17032eaceba8b2a3fe266a69bfb1e5977aec/web/app/components/header/account-setting/model-provider-page/model-provider-page-body.tsx).

Each provider card stores expansion per provider. Its model query is enabled
only while that provider is expanded; collapsed cards retain provider identity,
model-type badges, credential actions, and a show-models action. See
[`provider-added-card/index.tsx`](https://github.com/langgenius/dify/blob/d1fa17032eaceba8b2a3fe266a69bfb1e5977aec/web/app/components/header/account-setting/model-provider-page/provider-added-card/index.tsx).
Credential summaries distinguish working credentials, required API keys,
credit fallback/exhaustion, and destructive states using text plus a status dot
in
[`credential-panel.tsx`](https://github.com/langgenius/dify/blob/d1fa17032eaceba8b2a3fe266a69bfb1e5977aec/web/app/components/header/account-setting/model-provider-page/provider-added-card/credential-panel.tsx).

Dify also formalizes selection failures instead of collapsing them into one
unavailable state: active, configure-required, credits-exhausted,
API-key-unavailable, disabled, and incompatible are separate derived values in
[`derive-model-status.ts`](https://github.com/langgenius/dify/blob/d1fa17032eaceba8b2a3fe266a69bfb1e5977aec/web/app/components/header/account-setting/model-provider-page/derive-model-status.ts).
Its selector builds normalized provider/model search indexes and applies
feature predicates in
[`model-search.ts`](https://github.com/langgenius/dify/blob/d1fa17032eaceba8b2a3fe266a69bfb1e5977aec/web/app/components/header/account-setting/model-provider-page/model-selector/model-search.ts).
The popup gives its search and clear controls accessible names, bounds the
scrolling region, and labels that region in
[`popup-layout.tsx`](https://github.com/langgenius/dify/blob/d1fa17032eaceba8b2a3fe266a69bfb1e5977aec/web/app/components/header/account-setting/model-provider-page/model-selector/popup-layout.tsx).

**Patterns to use:** a collapsed card must still answer “what is this?”, “is it
ready?”, “what is selected?”, and “what action is needed?”; load expensive
catalog details on demand; make failures specific and actionable.

**Boundary for Nexa:** status text must come from Nexa's persisted connection,
availability, probe, and selection state. It must not be guessed from whether a
masked credential field happens to contain text in the frontend.

### 4. LibreChat: searchable settings and concise configured summaries

LibreChat's settings dialog switches at 767 px between a desktop side-by-side
view and a mobile list/detail flow. Search results remain directly available on
small screens rather than hiding behind the detail state. The implementation
uses established dialog and tab primitives in
[`Dialog.tsx`](https://github.com/danny-avila/LibreChat/blob/45cc53c40b47645b887c3bb996168e06aaa83f4c/client/src/components/Nav/Settings/Dialog.tsx).

Its sidebar gives the search input an accessible name, supports Escape to clear
the query, gives the clear action its own accessible name, and uses tab
semantics for navigation. See
[`Sidebar.tsx`](https://github.com/danny-avila/LibreChat/blob/45cc53c40b47645b887c3bb996168e06aaa83f4c/client/src/components/Nav/Settings/Sidebar.tsx)
and the normalized label/keyword matching in
[`search.ts`](https://github.com/danny-avila/LibreChat/blob/45cc53c40b47645b887c3bb996168e06aaa83f4c/client/src/components/Nav/Settings/search.ts).

Provider API keys are presented as compact rows with provider identity, a
truthful “not set” or expiry summary, and one configure/update action in
[`ProviderKeyRow.tsx`](https://github.com/danny-avila/LibreChat/blob/45cc53c40b47645b887c3bb996168e06aaa83f4c/client/src/components/Nav/SettingsTabs/ProviderKeys/ProviderKeyRow.tsx).

**Patterns to use:** settings search should be keyboard-clearable and retain
accessible navigation semantics; credential configuration should summarize
state without exposing secret material.

## Accessibility and responsive contract

The W3C disclosure pattern requires a button that toggles visibility, supports
Enter and Space, and exposes `aria-expanded`; `aria-controls` may identify the
controlled content. See the official
[`Disclosure (Show/Hide) Pattern`](https://www.w3.org/WAI/ARIA/apg/patterns/disclosure/).
Nexa's existing native buttons already supply Enter/Space behavior. The upgrade
should add stable trigger/panel IDs and `aria-controls`.

For a stack of related sections, the W3C accordion pattern keeps each trigger
in an appropriate heading, includes every interactive control in the normal tab
order, and ties the trigger to its panel with `aria-controls`. It also cautions
against creating excessive `region` landmarks when many panels can be open.
See the official
[`Accordion Pattern`](https://www.w3.org/WAI/ARIA/apg/patterns/accordion/).

Consequences for Nexa:

- The disclosure trigger should be one native button. Edit, delete, set-default,
  enable, and menu actions must be adjacent controls, not buttons nested inside
  the disclosure button.
- The collapsed summary must be textually available, not encoded only by color,
  opacity, a star, or an unlabeled dot.
- Focus must remain predictable when a section closes. If focus is inside the
  closing panel, move it to the trigger before hiding the panel.
- Do not add custom arrow-key focus management to independent disclosures.
  Normal Tab/Shift+Tab plus Enter/Space is the correct low-surprise behavior.
- Preserve the existing reduced-motion path and make chevrons `aria-hidden`
  when the button name already conveys the action/state.

WCAG 2.2's reflow guidance requires non-excepted horizontal-language content to
remain available without two-dimensional scrolling at a width equivalent to
320 CSS pixels. See
[`Understanding Success Criterion 1.4.10: Reflow`](https://www.w3.org/WAI/WCAG22/Understanding/reflow.html).
The current fixed `grid-cols-2` selector should therefore become one column at
narrow widths and add columns only at explicit breakpoints. Long provider/model
names, URLs, badges, and action groups must wrap or truncate with an accessible
full label; no action may disappear solely because the viewport narrowed.

## Nexa target interaction contract

### Information hierarchy

The AI Providers tab should render in this order:

1. A compact page header with Add provider and optional search/filter controls.
2. **Chat & reasoning providers**: configured rows first; each row shows name,
   provider, selected model, configured state, enabled state when supported,
   default state, and a single primary configure action.
3. **Image generation**: one compact category disclosure with provider/model,
   readiness, and credential-source summary. Opening it reveals the existing
   form directly, without a second redundant disclosure.
4. **Speech**: separate TTS and STT compact summaries within one category, while
   preserving explicit local/cloud identity and local runtime readiness.
5. **Advanced capability routing**: collapsed by default. Its summary shows
   connection/model/route counts plus any activation or load error; opening it
   renders `CapabilityRegistryPanel`.

If there are no configured chat providers, the empty state and Add provider
action remain visible without opening an accordion.

### Orthogonal state vocabulary

| State | Meaning | UI rule |
| --- | --- | --- |
| Configured | A persisted configuration exists and its required fields are present according to backend validation | Show “Configured”; do not expose or echo a secret |
| Enabled | Runtime use is allowed by the persisted connection/provider state | Show a switch only where this state is actually persisted |
| Default | This config is the current default selection | Show text plus icon; setting default must not enable or configure it implicitly |
| Available | The selected target is currently discoverable/callable/product-ready according to its authoritative availability state | Use the existing availability vocabulary and a reason when unavailable |
| Needs attention | A precise repair action exists: credentials missing, endpoint invalid, probe failed, model deprecated/unavailable, local runtime missing, or registry activation failed | State the reason and next action; never use only a red dot |

The frontend must not promote “configured” to “callable” or “product ready.”
Likewise, a saved default can be unavailable, and a configured provider can be
disabled. Keeping these distinctions prevents misleading summaries.

### Provider selection and filtering

- Search provider ID, localized name, description, and model aliases already
  present in the Nexa catalog. Normalize case and whitespace; avoid hidden
  fuzzy matches that are difficult to explain.
- Default ordering: configured, recommended for the current capability, then
  the remaining catalog. Within a group retain the catalog's stable order.
- Provide explicit filters for configured/all and local/cloud only if the
  result set warrants them. Do not merge local and cloud identity into one
  ambiguous provider state.
- Keep Custom/manual as a distinct final action. Unknown or edited endpoints
  must continue to use the custom trust/credential path.
- At narrow widths use a list view followed by a detail view with a labeled Back
  action; at wider widths a grid or list/detail layout is acceptable.

### Disclosure persistence

Disclosure state is presentation state, not provider configuration. It may be
kept in component state for the session. Collapsing a section must not:

- discard unsaved form edits;
- mark a form clean;
- save a form implicitly;
- cancel an in-flight connection test without reporting it;
- change enabled/default/configured state; or
- refetch a stable catalog on every open/close cycle.

An actionable error may force its category summary into a “Needs attention”
state, but should not repeatedly force the section open after the user closes
it.

## Explicit do / don't mapping

| Do | Do not |
| --- | --- |
| Reuse `Section`/`CollapsiblePanel`, reduced-motion utilities, catalog icons, and backend status types | Add a third accordion primitive with different keyboard behavior |
| Collapse the expert capability registry by default and keep a concise error/count summary visible | Put the entire registry projection above configured providers on every visit |
| Show configured, enabled, default, availability, and error as distinct text-bearing states | Use one star, color, or dot as a catch-all status |
| Keep row actions adjacent to the disclosure trigger | Nest edit/delete/default/toggle buttons inside a whole-card button |
| Use a one-column narrow layout and list-or-detail navigation where needed | Keep the fixed two-column catalog at 320 CSS px or hide actions on mobile |
| Search catalog-owned names/IDs/descriptions and expose a no-results state | Search or display credential values, or silently broaden to unrelated providers |
| Preserve local/cloud labels, endpoint trust, and shared-credential boundaries | Let a similar URL, redirect, edited endpoint, or visual grouping inherit credentials |
| Show only enabled controls backed by persisted state | Add decorative toggles that cannot survive reload |
| Keep Advanced reachable and summarize failures while collapsed | Hide expert diagnostics entirely or auto-expand them on every render |
| Independently implement the patterns under Nexa's design system | Copy upstream layouts, CSS, strings, icons, fixtures, or source |

## Acceptance tests

### Focused component tests

- Every disclosure trigger is a native button with a stable accessible name,
  correct `aria-expanded`, and `aria-controls` pointing at its panel.
- Enter and Space toggle each disclosure; Tab visits adjacent row actions in
  visual order; Escape clears provider search and returns the unfiltered list.
- Closing a panel containing focus moves focus to its trigger.
- Collapsed summaries expose configured count, chosen provider/model, default,
  enabled when supported, and precise error text without depending on color.
- Collapsing and reopening retains unsaved form fields and dirty state.
- Reduced-motion mode suppresses height/opacity motion while preserving state.

### Focused Playwright coverage

- AI Providers opens with configured chat providers and Add provider before the
  collapsed Advanced capability registry.
- Advanced summary reports counts and load/activation errors; opening it still
  exposes the existing registry mode controls.
- Provider search matches stable ID/name/description, has a labeled clear
  button, reports no results, and places configured/recommended entries first.
- At 320 CSS px equivalent width, cards become one column, long provider/model
  names and URLs do not cause page-level horizontal scrolling, and every action
  remains reachable.
- Desktop and narrow list/detail flows preserve the same selected provider and
  provide a labeled Back action.
- Configured, enabled, default, available, and needs-attention states can occur
  in independent combinations without changing one another.
- Existing exact-endpoint credential-reuse tests remain green for image, TTS,
  STT, custom HTTP, non-standard-port, and edited endpoints.
- Local TTS/STT remain visibly local and retain runtime/storage/deletion
  behavior; cloud speech remains visibly cloud.
- Full keyboard-only add, edit, test, save, set-default, disable, delete, expand,
  collapse, search, clear, and Back flows complete without a focus trap.

## License and integration boundary

| Project | License at reviewed revision | Nexa boundary |
| --- | --- | --- |
| Open WebUI | Custom [`Open WebUI License`](https://github.com/open-webui/open-webui/blob/01f4282f1ffe0d6212f58d3afbeae21fffd0c4be/LICENSE), including conditions beyond a standard permissive license | Architectural comparison only; do not copy Svelte, styles, words, branding, or layout |
| LobeChat | [`LobeHub Community License`](https://github.com/lobehub/lobe-chat/blob/ca27228d55bb604f8bcf455a18f5e87d3ba9f9a5/LICENSE), based on Apache-2.0 with additional derivative-work/commercial conditions | Architectural comparison only; independently implement state grouping, search, and responsive navigation |
| Dify | [Modified Apache-2.0 license with additional multi-tenant/branding conditions and an interactive-design patent notice](https://github.com/langgenius/dify/blob/d1fa17032eaceba8b2a3fe266a69bfb1e5977aec/LICENSE) | Architectural comparison only; do not reproduce its card appearance, source, CSS, strings, icons, or patented interaction design |
| LibreChat | [`MIT`](https://github.com/danny-avila/LibreChat/blob/45cc53c40b47645b887c3bb996168e06aaa83f4c/LICENSE) | Concepts may inform an independent implementation; copying code would still require copyright/notice compliance |

This note authorizes no source copying and adds no runtime dependency. If a
future implementation copies or closely adapts upstream material, it requires a
separate license review and preservation of all applicable notices. The safe
path for this PR is to implement the decisions above using Nexa's existing
React components, Tailwind tokens, i18n catalog, backend status contracts, and
tests.
