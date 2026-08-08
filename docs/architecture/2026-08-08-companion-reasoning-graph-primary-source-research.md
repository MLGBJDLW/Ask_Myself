# Desktop Companion, Codex Pets, DeepSeek Replay, and Graph Rendering: Primary-source Research

This note supports the 2026-08-08 upgrade requested in `D:\Nexa.txt`. It
records the upstream contracts that must remain true while Nexa fixes the graph
artifact, removes synthetic DeepSeek reasoning, and builds a real desktop
companion. It is an implementation input, not permission to copy another
project's source, assets, product design, or wording.

## Executive decisions

1. Build the desktop pet as one independent Tauri webview window owned by the
   application, not as a child of the main window and not as a larger version
   of `CompanionPet.tsx`. Create it hidden during `setup`, retain its label for
   the process lifetime, and use show/hide for ordinary lifecycle changes.
2. Give click-through an explicit runtime state. Tauri's public API toggles
   cursor events for the whole window; a fully click-through pet therefore
   needs an out-of-window recovery action such as the tray menu. Do not rely on
   a hover callback that can no longer fire after click-through is enabled.
3. Persist a monitor-relative placement record, then resolve and clamp it to
   the current monitor work area on every restore. Tauri exposes monitor
   geometry in physical pixels while creation coordinates are logical pixels;
   raw `x/y` values alone are not portable across DPI or topology changes.
4. Treat Codex pet formats as versioned dialects. The current public
   `openai/codex` TUI source accepts a strict 1536x1872, 8x9 package and legacy
   `avatar.json`; Codex Desktop V2 evidence describes a 1536x2288, 8x11 package
   with `spriteVersionNumber: 2`. A V2 desktop package is not accepted by the
   current open-source TUI loader. Nexa must detect the dialect rather than
   silently rewriting one into the other.
5. Intercept `/pet` and `/pets` locally. The current Codex source makes `/pets`
   canonical and `/pet` an alias, with picker, named-selection, and hide/disable
   behavior. These commands are UI configuration and must never be sent to a
   model.
6. For official DeepSeek V4 OpenAI-format requests, persist and replay the
   complete assistant tuple `content + reasoning_content + tool_calls` whenever
   the assistant made a tool call. Synthetic reasoning text cannot satisfy the
   contract. If required reasoning was not captured, recovery must happen
   before executing the tool or persisting a replayable chain.
7. Fix the graph rectangle at the SVG filter graph, not by shrinking the
   filter rectangle. `feTurbulence` covers its primitive region; the grain must
   be composited through `SourceAlpha` before blending with the node. Dark and
   light graph colors should then come from one semantic token set, following
   React Flow's root color-mode/CSS-variable pattern.

## Scope, evidence quality, and reviewed revisions

| Source | Revision or review date | What was inspected | Evidence status |
| --- | --- | --- | --- |
| Tauri | [`7cd71369c00978a3783b6ae3e9972358abbe4ae6`](https://github.com/tauri-apps/tauri/commit/7cd71369c00978a3783b6ae3e9972358abbe4ae6), tag `tauri-v2.11.5` | Webview window construction, platform limitations, cursor-event control, monitor geometry and events | Normative for Nexa's pinned Tauri version |
| OpenAI Codex public source | [`3aae5d885bac39c1262491aa3fd100dfd8b3919f`](https://github.com/openai/codex/commit/3aae5d885bac39c1262491aa3fd100dfd8b3919f) | Current TUI pet loader, manifest, cache, picker, slash commands, legacy paths | Normative for the open-source Codex TUI only |
| OpenAI Codex issue tracker | Reviewed 2026-08-08 | Desktop V1/V2 package observations, overlay/DPI/path regressions | First-party tracker evidence, not a stable public Desktop API specification |
| DeepSeek API docs | Reviewed 2026-08-08 | V4 model IDs, official endpoints, thinking-mode replay and tool calls | Normative for official DeepSeek endpoints |
| xyflow / React Flow | [`ee40209955e2e3b3d738397d281a147a73154fbd`](https://github.com/xyflow/xyflow/commit/ee40209955e2e3b3d738397d281a147a73154fbd) | Color-mode selection and semantic CSS variables for graph primitives | Primary open-source implementation reference |
| W3C Filter Effects Level 1 | Current specification reviewed 2026-08-08 | Filter regions, primitive subregions, `feTurbulence`, `SourceAlpha`, `feComposite` | Normative web-platform specification |

The public `openai/codex` repository does not contain the Codex Desktop overlay
renderer or the bundled Desktop `hatch-pet` contract referenced by issue
reports. Consequently, this note calls the open-source TUI behavior
**confirmed**, but calls Desktop V2 behavior **compatibility evidence**. Nexa
must not claim byte-for-byte Desktop V2 compatibility without a fixture tested
against a current Codex Desktop build.

## Current Nexa seams

- `apps/desktop/src/components/chat/CompanionPet.tsx` is a task-state button
  inside the chat page. It is useful as an optional status projection, but it
  cannot own desktop coordinates, window z-order, monitor changes, or process-
  global companion state.
- `apps/desktop/src-tauri/tauri.conf.json` currently declares the main window;
  `apps/desktop/src-tauri/src/main.rs` is the process/window lifecycle seam.
  Nexa currently pins `tauri = 2.11.5`.
- `crates/core/src/companion.rs` already owns semantic activity projection and
  Nexa's first package validation vocabulary. The new desktop renderer should
  consume that state rather than re-derive activity from `ChatPage` props.
- `crates/core/src/llm/openai.rs` has both a real
  `reasoning_content` serializer and a synthetic-missing-history path. The
  latter is incompatible with DeepSeek's requirement to replay the model's
  actual field.
- `apps/desktop/src/components/knowledge/KnowledgeGraphView.tsx` is a custom
  SVG graph. The affected node filter combines turbulence, flood, blend, and
  drop shadow, so the filter graph itself is the first repair surface.

## 1. Tauri v2.11.5 desktop companion window contract

### 1.1 Construction and ownership

Tauri's `WebviewWindowBuilder` directly supports the properties needed for an
overlay: undecorated/transparent rendering, initial visibility and focus,
always-on-top, taskbar visibility, shadow, and workspace visibility. The exact
builder implementation is in the pinned
[`webview_window.rs`](https://github.com/tauri-apps/tauri/blob/7cd71369c00978a3783b6ae3e9972358abbe4ae6/crates/tauri/src/webview/webview_window.rs#L54-L159)
and the property methods are in the
[same file](https://github.com/tauri-apps/tauri/blob/7cd71369c00978a3783b6ae3e9972358abbe4ae6/crates/tauri/src/webview/webview_window.rs#L562-L615).

The builder carries an explicit Windows warning: constructing a webview window
from a synchronous command or synchronous event handler can deadlock; Tauri's
examples create in `setup`, a separate thread, or an async command. Nexa should
therefore construct the single `companion` window during `setup` with
`visible(false)`, or use an async command only if construction truly must be
lazy. Ordinary enable/disable changes should show/hide the retained window,
not repeatedly destroy and recreate WebView2.

Do not set the main window as `parent`. Tauri documents that a Windows owned
window is hidden when its owner is minimized and destroyed when its owner is
destroyed; this conflicts with a pet that survives main-window minimization or
close-to-tray. The parent behavior is documented in
[`WebviewWindowBuilder::parent`](https://github.com/tauri-apps/tauri/blob/7cd71369c00978a3783b6ae3e9972358abbe4ae6/crates/tauri/src/webview/webview_window.rs#L617-L643).

Recommended creation contract:

```rust
WebviewWindowBuilder::new(app, "companion", WebviewUrl::App("index.html?window=companion".into()))
    .transparent(true)
    .decorations(false)
    .shadow(false)
    .resizable(false)
    .maximizable(false)
    .minimizable(false)
    .focused(false)
    .skip_taskbar(true)
    .always_on_top(true)
    .visible(false)
```

This is a Nexa projection of the documented flags, not copied Tauri code.
`shadow(false)` is important: Tauri states that enabling a shadow on an
undecorated Windows window adds a one-pixel white border and Windows 11 rounded
corners, exactly the wrong visual for a transparent sprite. See the pinned
[shadow documentation](https://github.com/tauri-apps/tauri/blob/7cd71369c00978a3783b6ae3e9972358abbe4ae6/crates/tauri/src/webview/webview_window.rs#L602-L615).

Platform capabilities must remain explicit:

- `skip_taskbar` is unsupported on macOS.
- `visible_on_all_workspaces` is unsupported on Windows, iOS, and Android in
  the underlying Window builder, so Nexa must not show it as a working Windows
  preference. See the pinned
  [`WindowBuilder` implementation](https://github.com/tauri-apps/tauri/blob/7cd71369c00978a3783b6ae3e9972358abbe4ae6/crates/tauri/src/window/mod.rs)
  and Tauri's [2.11.5 API reference](https://docs.rs/tauri/2.11.5/tauri/window/struct.WindowBuilder.html#method.visible_on_all_workspaces).
- Transparent webviews on macOS require `macOSPrivateApi`; Tauri warns that the
  private API prevents App Store acceptance. The pinned configuration source
  states both constraints in
  [`config.rs`](https://github.com/tauri-apps/tauri/blob/7cd71369c00978a3783b6ae3e9972358abbe4ae6/crates/tauri-utils/src/config.rs#L2020-L2025).

### 1.2 Lifecycle state machine

Use one application-level controller, not React component lifetime:

```text
Uninitialized -> HiddenReady -> VisibleInteractive
                           \-> VisibleLocked
                           \-> VisiblePassThrough
any visible state -> HiddenReady
process shutdown -> Destroyed
```

- `HiddenReady` retains decoded pack metadata and the webview window.
- Main-window hide, minimization, chat navigation, and project switching do not
  alter the companion lifecycle.
- Window `CloseRequested` for the companion should normally prevent close and
  transition to `HiddenReady`; an explicit application shutdown may destroy it.
- Reassert desired `always_on_top` when showing and after platform resume. The
  desired value belongs to settings; the native z-order result is runtime
  state and can fail independently.
- Keep a tray action for show/hide, unlock, reset position, and open settings.
  It remains reachable even when the companion ignores all cursor events.

Tauri exposes `on_window_event` on the retained window handle, so native moved,
resized, scale and close events can be centralized outside React; see
[`WebviewWindow::on_window_event`](https://github.com/tauri-apps/tauri/blob/7cd71369c00978a3783b6ae3e9972358abbe4ae6/crates/tauri/src/webview/webview_window.rs#L1523-L1526).

### 1.3 Click-through and dragging

The public API is a boolean whole-window operation:
[`set_ignore_cursor_events`](https://github.com/tauri-apps/tauri/blob/7cd71369c00978a3783b6ae3e9972358abbe4ae6/crates/tauri/src/webview/webview_window.rs#L2132-L2139)
and its TypeScript wrapper
[`setIgnoreCursorEvents`](https://github.com/tauri-apps/tauri/blob/7cd71369c00978a3783b6ae3e9972358abbe4ae6/packages/api/src/window.ts#L1659-L1675).
It does not expose an alpha-mask or DOM-element-level native hit-test region.

Nexa should therefore define truthful modes:

| Mode | Native cursor events | Drag | Click action | Recovery |
| --- | --- | --- | --- | --- |
| Interactive | Processed | Enabled from the sprite/drag handle | Open bound task or task center | Pet or tray |
| Locked | Processed | Disabled | Still available | Pet or tray |
| Pass-through | Ignored for the whole window | Impossible while active | None | Tray/global shortcut/settings only |

“Transparent background passes through but sprite remains interactive” is not
provided by the cited Tauri API. It would require an additional native hit-test
implementation or cursor-position polling that toggles the whole window from
outside the ignored webview. Do not present that behavior as implemented by
`setIgnoreCursorEvents` alone.

### 1.4 Multi-monitor and DPI persistence

Tauri's `Monitor` contract includes name, physical position, physical size,
physical work area, and scale factor. It explicitly warns that window-creation
`x/y/width/height` are logical pixels and must be converted from monitor
physical coordinates. See the pinned
[`Monitor` interface](https://github.com/tauri-apps/tauri/blob/7cd71369c00978a3783b6ae3e9972358abbe4ae6/packages/api/src/window.ts#L42-L90).
Tauri also exposes
[`currentMonitor`](https://github.com/tauri-apps/tauri/blob/7cd71369c00978a3783b6ae3e9972358abbe4ae6/packages/api/src/window.ts#L2591-L2605),
[`availableMonitors`](https://github.com/tauri-apps/tauri/blob/7cd71369c00978a3783b6ae3e9972358abbe4ae6/packages/api/src/window.ts#L2641-L2655),
physical moved events, and scale-change events caused by moving between
different-DPI displays
([`onMoved`](https://github.com/tauri-apps/tauri/blob/7cd71369c00978a3783b6ae3e9972358abbe4ae6/packages/api/src/window.ts#L1881-L1903),
[`onScaleChanged`](https://github.com/tauri-apps/tauri/blob/7cd71369c00978a3783b6ae3e9972358abbe4ae6/packages/api/src/window.ts#L2064-L2090)).

Persist something equivalent to:

```text
monitorNameHint
monitorPhysicalSizeHint
anchorXRatioWithinWorkArea
anchorYRatioWithinWorkArea
lastPhysicalPosition
lastScaleFactor
petScale
```

Restore algorithm:

1. Enumerate current monitors.
2. Match the saved monitor using stable available evidence (name plus geometry
   is a hint, not a guaranteed hardware ID).
3. Fall back to the primary monitor if no match exists.
4. Reconstruct from ratios inside the selected monitor's current physical work
   area.
5. Clamp the entire companion window into that work area with a visible margin.
6. Convert to logical coordinates only at the window-creation/set-position
   boundary required by the called API.
7. Re-run on show, moved display/scale change, resume, and an explicit reset.

Negative desktop coordinates are valid in multi-monitor layouts; do not clamp
to zero before selecting a monitor. The current Tauri API explicitly notes
that desktop cursor coordinates can be negative and that desktop origin differs
between Windows/macOS and X11 in
[`cursorPosition`](https://github.com/tauri-apps/tauri/blob/7cd71369c00978a3783b6ae3e9972358abbe4ae6/packages/api/src/window.ts#L2657-L2666).

Codex Desktop issue
[#22534](https://github.com/openai/codex/issues/22534) reports valid 8x9 pets
collapsing to a vertical line or disappearing after a Thunderbolt dock/external
monitor change. This is not proof of a Tauri defect, but it is strong product
evidence that dock, GPU and DPI transitions belong in Nexa's test matrix.

## 2. Current openai/codex pet packages and commands

### 2.1 Confirmed open-source TUI format

At the reviewed Codex revision, the TUI catalog defines 192x208 cells, eight
columns, nine rows, and therefore an exact 1536x1872 spritesheet. See
[`catalog.rs`](https://github.com/openai/codex/blob/3aae5d885bac39c1262491aa3fd100dfd8b3919f/codex-rs/tui/src/pets/catalog.rs#L1-L22).

The canonical custom package is:

```text
$CODEX_HOME/
  pets/
    <pet-id>/
      pet.json
      spritesheet.webp        # or the manifest-relative child path
```

The current loader also scans the legacy form:

```text
$CODEX_HOME/
  avatars/
    <pet-id>/
      avatar.json
      spritesheet.webp
```

Both paths are confirmed by
[`model.rs`](https://github.com/openai/codex/blob/3aae5d885bac39c1262491aa3fd100dfd8b3919f/codex-rs/tui/src/pets/model.rs#L145-L181)
and the picker scan in
[`picker.rs`](https://github.com/openai/codex/blob/3aae5d885bac39c1262491aa3fd100dfd8b3919f/codex-rs/tui/src/pets/picker.rs#L129-L177).

Supported public manifest fields are:

```json
{
  "id": "optional-id",
  "displayName": "Display name",
  "description": "Optional description",
  "spritesheetPath": "spritesheet.webp",
  "frame": {
    "width": 192,
    "height": 208,
    "columns": 8,
    "rows": 9
  },
  "animations": {
    "idle": {
      "frames": [0, 1, 2],
      "fps": 8,
      "loop": true,
      "fallback": "idle"
    }
  }
}
```

`frame` and `animations` are optional. Defaults provide the familiar nine rows:
idle, right movement, left movement, waving, jumping, failed, waiting, running,
and review, plus legacy aliases such as `move_right`, `wave`, `bounce`, and
`sad`. The schema and normalization are in
[`model.rs`](https://github.com/openai/codex/blob/3aae5d885bac39c1262491aa3fd100dfd8b3919f/codex-rs/tui/src/pets/model.rs#L91-L131),
and the default state mapping is in the
[same source](https://github.com/openai/codex/blob/3aae5d885bac39c1262491aa3fd100dfd8b3919f/codex-rs/tui/src/pets/model.rs#L365-L485).

The open-source loader enforces useful boundaries Nexa should preserve:

- The spritesheet path must be lexically relative: absolute paths, Windows
  prefixes, and parent (`..`) components are rejected. Nexa should additionally
  canonicalize the opened asset and package root if symlinks/reparse points are
  in scope, because the cited Codex check is component-based rather than a
  canonical-path identity check.
- The decoded image must be exactly 1536x1872 even when a custom frame grid is
  supplied.
- Frame dimensions/counts must be nonzero, the grid must exactly cover the
  spritesheet, and total frames are capped at 256.
- Every animation must contain frames inside the grid; FPS must be finite,
  positive, and no greater than 60; fallbacks must name an existing animation.

See the pinned validation implementation in
[`model.rs`](https://github.com/openai/codex/blob/3aae5d885bac39c1262491aa3fd100dfd8b3919f/codex-rs/tui/src/pets/model.rs#L219-L319).
Built-in pets use a separate versioned CDN/cache path, HTTPS-only downloads, a
four-megabyte download cap, geometry validation, staging, and rename. See
[`asset_pack.rs`](https://github.com/openai/codex/blob/3aae5d885bac39c1262491aa3fd100dfd8b3919f/codex-rs/tui/src/pets/asset_pack.rs).

### 2.2 Desktop V1/V2 compatibility evidence and its limit

Codex Desktop issues in the official repository consistently describe V1 as
`pet.json` plus a 1536x1872 RGBA WebP atlas, eight columns by nine rows with
192x208 cells. For example, issue
[#22534](https://github.com/openai/codex/issues/22534)
records that exact installed layout across multiple validated packages.

Issue [#34240](https://github.com/openai/codex/issues/34240) records the newer
Desktop V2 contract shipped with the bundled `hatch-pet` workflow:

- `pet.json` contains `"spriteVersionNumber": 2`;
- `spritesheet.webp` is 1536x2288 RGBA;
- the grid is 8x11 using the same 192x208 cells;
- rows 0-8 are the standard states;
- rows 9-10 contain 16 clockwise look directions.

That issue also reports that V2 look rows were accepted but not activated in a
specific Desktop build. This is why Nexa must distinguish “package parses”
from “runtime behavior is available.” An imported V2 package should retain its
version and report capability/preview results, not silently downgrade it.

Crucially, the reviewed open-source TUI `PetFile` has no
`spriteVersionNumber` field and its dimension validator rejects anything other
than 1536x1872. Therefore:

| Dialect | Manifest/version | Atlas | Confirmed consumer |
| --- | --- | --- | --- |
| Codex TUI/current V1-shaped package | `pet.json`, optional `frame` and `animations`; no public version field | exactly 1536x1872, 8x9 by default | Current public `openai/codex` TUI |
| Codex legacy avatar | `avatar.json` | same exact TUI geometry | Current public TUI compatibility loader |
| Codex Desktop V1 | `pet.json`, no V2 marker in reported examples | 1536x1872, 8x9 | Desktop issue evidence |
| Codex Desktop V2 | `pet.json` with `spriteVersionNumber: 2` | 1536x2288, 8x11 | Desktop bundled-skill/issue evidence only |

Nexa should implement `codex_tui_v1`, `codex_desktop_v1`, and
`codex_desktop_v2` as separate projections even when some fields overlap.

### 2.3 Current command behavior

The public TUI makes `pets` the canonical slash command and `pet` an alias in
[`slash_command.rs`](https://github.com/openai/codex/blob/3aae5d885bac39c1262491aa3fd100dfd8b3919f/codex-rs/tui/src/slash_command.rs#L51-L59).
The current tests establish these behaviors:

- `/pets` opens a searchable picker;
- `/pets <id>` selects a named pet;
- `/pets disable` disables pets;
- `/pet hide` is an alias for disabling/hiding;
- command handling emits local application events and no model operation.

See the pinned tests in
[`slash_commands.rs`](https://github.com/openai/codex/blob/3aae5d885bac39c1262491aa3fd100dfd8b3919f/codex-rs/tui/src/chatwidget/tests/slash_commands.rs#L2440-L2499)
and picker behavior in
[`picker.rs`](https://github.com/openai/codex/blob/3aae5d885bac39c1262491aa3fd100dfd8b3919f/codex-rs/tui/src/pets/picker.rs#L1-L127).
Codex issue [#20836](https://github.com/openai/codex/issues/20836) documents a
Desktop regression where `/pet` escaped local command handling and became a
normal chat message, reinforcing that Nexa needs a parser-level interception
test.

Recommended Nexa compatibility surface:

```text
/pet                  open companion picker/status
/pets                 alias of /pet
/pet <id>             select a validated installed package
/pet show             show retained desktop window
/pet hide             hide retained desktop window
/pet disable          disable companion runtime
/pet reset            reset placement into current monitor work area
```

Only behavior confirmed above should be advertised as Codex-compatible;
Nexa-only extensions should be labeled as such.

Windows/WSL paths need a canonical native owner. Issue
[#21471](https://github.com/openai/codex/issues/21471) reports a Windows Desktop
custom-pet load failure when `CODEX_HOME` came from a WSL app-server path. Nexa
should not concatenate path strings across runtimes: discover the directory in
the process that owns the files, canonicalize it there, and return a structured
diagnostic when it is not accessible.

## 3. DeepSeek V4 `reasoning_content` and tool-call replay

### 3.1 Official endpoint and model boundary

The current DeepSeek guides use the OpenAI-compatible endpoint
`https://api.deepseek.com`, the Anthropic-compatible endpoint
`https://api.deepseek.com/anthropic`, and current model IDs
`deepseek-v4-pro` and `deepseek-v4-flash`. See the official
[`Thinking Mode`](https://api-docs.deepseek.com/zh-cn/guides/thinking_mode/)
examples and
[`GitHub Copilot CLI integration`](https://api-docs.deepseek.com/quick_start/agent_integrations/copilot_cli/).
These rules belong to exact official endpoints plus the selected API dialect;
a model name seen through an arbitrary OpenAI-compatible router is not proof
that the router preserves the same field contract.

### 3.2 Normative replay rules

The official [Thinking Mode](https://api-docs.deepseek.com/zh-cn/guides/thinking_mode/)
guide states:

1. Thinking output is returned in `reasoning_content` alongside `content`.
2. If an assistant did not make a tool call between user messages, its prior
   reasoning need not be returned and is ignored if it is returned.
3. If an assistant did make a tool call, its complete `reasoning_content` must
   be returned in all subsequent requests; missing replay produces HTTP 400.
4. The response assistant message already contains the required
   `content`, `reasoning_content`, and `tool_calls`, and the official sample
   appends that message before appending tool results.

The tool-call section currently uses an even more conservative formulation for
requests carrying a `tools` parameter: replay the complete reasoning in every
subsequent request. The safest official-endpoint implementation therefore
retains all reasoning emitted inside a tool-enabled loop, even though the
high-level multi-turn rule distinguishes assistants that actually called a
tool from those that did not.

The concrete wire unit is therefore:

```text
assistant {
  content,
  reasoning_content,   # exact provider output when required
  tool_calls
}
tool { tool_call_id, content }
... possibly more assistant/tool sub-turns ...
assistant final
```

The complete unit remains replayable across the next user turn. Editing,
deleting, or compaction must either keep it intact or replace the whole unit
with a new summary/replay boundary; retaining a tool result while dropping its
assistant/tool-call/reasoning predecessor is invalid.

DeepSeek's official
[`GitHub Copilot CLI integration`](https://api-docs.deepseek.com/quick_start/agent_integrations/copilot_cli/)
is especially useful evidence: it directs users to the Anthropic endpoint
because Copilot's OpenAI integration cannot replay `reasoning_content` and
otherwise receives the same 400. That confirms this is an API-dialect
capability, not merely a model-name switch.

### 3.3 Consequences for Nexa

The literal sentinel

```text
[reasoning content unavailable in local history]
```

is not provider output and cannot satisfy “fully pass back
`reasoning_content`.” It should never be created, persisted, displayed as model
reasoning, or serialized outbound.

Nexa needs separate concerns:

```rust
struct ReasoningEnvelope {
    display_text: Option<String>,
    replay_payload: Option<ProviderReasoningPayload>,
    capture_status: ReasoningCaptureStatus,
    replay_policy: ReasoningReplayPolicy,
    provider_id: String,
    endpoint_id: String,
    model_id: String,
    api_dialect: ApiDialect,
}
```

`display_text` is a UI decision. `replay_payload` is provider protocol state;
future providers may require structured or signed blocks rather than a string.
Never reconstruct replay state from what the UI chose to show.

Fail-closed recovery is an inference from the official 400 contract:

```text
official DeepSeek OpenAI endpoint
AND thinking enabled
AND assistant has tool_calls
AND captured reasoning_content is absent/incomplete
=> do not execute the tool yet
```

At that point Nexa may retry the model step using a supported recovery path
before any side effect. If it cannot obtain a complete replayable response, it
should end with a structured recoverable error. Fabricating reasoning or
executing the tool and hoping a later request succeeds creates an unreplayable
history and is not supported by the cited contract.

Legacy rows containing the exact sentinel should be migrated idempotently to a
missing/legacy capture status and removed from display/outbound fields. Since
the original provider payload is unrecoverable, the migration must not claim
to repair replay; it must establish a boundary before the broken tool chain.

Required fixtures:

1. streaming V4 reasoning plus one and multiple tool calls;
2. non-stream V4 reasoning plus tool calls;
3. official endpoint tool call with missing reasoning fails before dispatch;
4. no-tool assistant with no reasoning remains valid;
5. full assistant tuple survives another user turn;
6. compaction never splits assistant/tool replay units;
7. exact legacy sentinel migration and DTO/UI filtering;
8. custom/router endpoint does not inherit official DeepSeek policy from its
   model name;
9. Anthropic endpoint uses Anthropic block semantics, not
   `reasoning_content` synthesis;
10. repository-wide assertion that no new sentinel is emitted or serialized.

## 4. Knowledge graph dark theme and SVG filter region

### 4.1 Why turbulence produces a rectangular artifact

The W3C [Filter Effects Level 1](https://www.w3.org/TR/filter-effects-1/)
specification states that a primitive with no referenced input, including
`feTurbulence`, defaults its primitive subregion to the entire filter region.
It also distinguishes the filter region/primitive region (hard clipping
rectangles) from the pixels' alpha/content. Enlarging `x/y/width/height` can
prevent glow clipping, but it does not make full-region turbulence transparent.

The same specification demonstrates the correct masking operation:
`feComposite` with the original `SourceAlpha` limits an intermediate effect to
the source graphic. Therefore the safe frost pipeline is:

```xml
<feTurbulence ... result="noise" />
<feColorMatrix in="noise" ... result="grain-mask" />
<feFlood flood-color="var(--graph-node-frost)" result="grain-color" />
<feComposite in="grain-color" in2="grain-mask" operator="in" result="grain" />
<feComposite in="grain" in2="SourceAlpha" operator="in" result="masked-grain" />
<feBlend in="SourceGraphic" in2="masked-grain" mode="screen" result="frosted" />
```

The `SourceAlpha` composite is the important repair. Filter-region expansion
may still be needed for shadows, but it is not the white-rectangle fix.
Do not add a white stroke as a visual workaround; that creates a second dark-
theme defect and obscures whether the alpha mask is correct.

### 4.2 Theme semantics from React Flow

React Flow's official theming guide uses a root `colorMode` (`light`, `dark`,
or `system`) and recommends CSS variables for graph primitives. See the
official [Theming guide](https://reactflow.dev/learn/customization/theming).
The pinned source implements a complete dark variable set for edges,
background, minimap, and patterns in
[`init.css`](https://github.com/xyflow/xyflow/blob/ee40209955e2e3b3d738397d281a147a73154fbd/packages/system/src/styles/init.css#L1-L55)
and separate node/control/label values in
[`style.css`](https://github.com/xyflow/xyflow/blob/ee40209955e2e3b3d738397d281a147a73154fbd/packages/system/src/styles/style.css#L1-L69).
Its `system` mode follows `prefers-color-scheme` and updates when the media
query changes in
[`useColorModeClass.ts`](https://github.com/xyflow/xyflow/blob/ee40209955e2e3b3d738397d281a147a73154fbd/packages/react/src/hooks/useColorModeClass.ts).

The reusable pattern is semantic ownership, not React Flow itself:

```text
--graph-canvas-background
--graph-canvas-grid
--graph-edge-default
--graph-edge-selected
--graph-node-border
--graph-node-frost
--graph-label-background
--graph-label-border
--graph-label-text
--graph-shadow-color
--graph-shadow-opacity
```

All SVG presentation attributes and filter flood/drop-shadow colors should
resolve through these tokens. A dark selector changes the token values once;
node selection, drag, agent-used highlighting, focus mode, and relation
categories remain orthogonal states. Keep deliberate semantic category colors,
but remove incidental `white`, `#fff`, or `stroke-white` literals from graph
nodes/labels.

### 4.3 Regression contract

The focused visual regression should load a deterministic graph fixture and
capture both light and dark themes at the same viewport. It must cover:

- unselected, selected, dragged, and agent-used nodes;
- every graph mode that uses the node filter;
- label chips and relation-count badges;
- empty canvas and edge glow;
- reduced motion, so the screenshot does not depend on animation phase.

In addition to snapshot comparison, inspect representative filtered node
bounding boxes: pixels outside the circle plus expected shadow/glow must stay
at the canvas color rather than becoming a uniform bright rectangle. This
directly tests the `SourceAlpha` invariant instead of accepting a broadly
changed screenshot.

## 5. Integration boundaries and delivery order

Recommended implementation order follows the independent contracts:

1. Graph SVG filter/theme hotfix and dark visual regression.
2. Stop sentinel generation/display/outbound serialization and migrate exact
   legacy data.
3. Add the provider-neutral reasoning envelope/replay boundary and DeepSeek
   endpoint-specific fixtures.
4. Add companion settings and versioned pack registry/adapters.
5. Add the independent Tauri overlay window and application-level controller.
6. Add local `/pet`/`/pets` behavior and Codex package/path compatibility.
7. Run cross-platform/DPI/dock/resume hardening.

The first two are user-visible hotfixes and do not need to wait for the larger
companion runtime. The companion pack registry should land before the overlay
so importing/validation can be tested independently of native window behavior.

## 6. License and reuse boundary

- Tauri is dual-licensed under
  [Apache-2.0](https://github.com/tauri-apps/tauri/blob/7cd71369c00978a3783b6ae3e9972358abbe4ae6/LICENSE_APACHE-2.0)
  and [MIT](https://github.com/tauri-apps/tauri/blob/7cd71369c00978a3783b6ae3e9972358abbe4ae6/LICENSE_MIT).
- The reviewed public
  [OpenAI Codex source](https://github.com/openai/codex/blob/3aae5d885bac39c1262491aa3fd100dfd8b3919f/LICENSE)
  is Apache-2.0. This does not make the unpublished Codex Desktop renderer or
  bundled pet assets part of the public source license surface.
- [xyflow](https://github.com/xyflow/xyflow/blob/ee40209955e2e3b3d738397d281a147a73154fbd/LICENSE)
  is MIT-licensed.

Nexa only needs the documented contracts and general architectural patterns.
Implement the window controller, pack adapters, replay state, token names, and
tests independently; do not copy Codex pet assets, Desktop bundle code, React
Flow styling, or product wording.

## 7. Acceptance checklist derived from the sources

### Companion

- The pet remains alive when the main window hides/minimizes/closes to tray.
- The overlay has no taskbar entry where supported, does not take initial
  focus, has no Windows shadow border, and truthfully reports unsupported
  workspace/taskbar features per platform.
- Interactive, locked, and pass-through modes have distinct persisted state;
  pass-through always has an external unlock path.
- Position restore clamps the entire window to the current physical work area
  across 100/125/150/200% DPI, negative monitor coordinates, dock attach/detach,
  display reorder, sleep/resume, and reset.
- `setup` or an async path owns window construction; no synchronous Windows
  command/event handler creates WebView2.

### Pet packages and commands

- V1 8x9, legacy avatar, and Desktop V2 8x11 are separately detected and
  validated; an incompatible consumer is reported, not silently converted.
- Manifest-relative assets cannot escape the pack directory; decoded geometry,
  byte size, frame count, animation indices/FPS, and fallback names are bounded.
- `/pet` and `/pets` are intercepted locally; picker, named selection, hide,
  disable, and reset do not create a user/model message.
- Windows and WSL paths are normalized by the owning runtime and errors include
  the resolved source without exposing arbitrary file contents.

### DeepSeek

- Official V4 tool-call assistants preserve the exact provider
  `reasoning_content` and replay it with `content` and `tool_calls`.
- No synthetic sentinel appears in new data, UI, logs, or outbound requests.
- Missing required reasoning blocks tool dispatch and yields recovery/error
  before any side effect.
- Router/custom endpoints and Anthropic-format endpoints use their own explicit
  capability/dialect contract.
- Compaction and edit/delete operations preserve or deliberately boundary the
  complete assistant/tool replay unit.

### Graph

- Turbulence grain is masked with `SourceAlpha`; expanded filter regions only
  provide shadow/glow headroom.
- Dark and light SVG colors come from semantic graph tokens; there is no
  incidental hard-white node/label stroke.
- Deterministic dark/light visual coverage includes selected, dragged,
  agent-used, reduced-motion, and all graph modes, with a focused no-rectangle
  pixel assertion.
