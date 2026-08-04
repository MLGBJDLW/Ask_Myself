# Turn timing, file visuals, motion, Task Center, and prompt caching: primary-source notes

Date: 2026-08-04

This note validates the five upgrade areas in `D:\Nexa.txt` against the current Nexa baseline, first-party specifications and documentation, and immutable source snapshots from leading open-source projects. It is an engineering input, not an implementation record. Recommendations are explicitly labeled as Nexa design inferences; protocol and library facts link to the source that owns them.

## Executive decision

1. **Give a turn one durable timing contract, but let only the small elapsed-time view tick.** Store lifecycle timestamps and phase transitions; calculate the live display from a monotonic clock while the WebView is alive and from persisted wall-clock timestamps after restart. The current private launch timer is not a durable turn contract. [W3C High Resolution Time clock guidance](https://www.w3.org/TR/hr-time-3/#sec-clocks), [OpenAI Codex turn lifecycle](https://github.com/openai/codex/blob/8e3b5d3e875fd52f1edff75e2f055e4990e866c0/codex-rs/app-server/README.md#L1497-L1509)
2. **Model file visuals separately from badge tone.** A low-saturation Nexa badge shell plus a brand accent is the safe default; selected high-frequency types can use audited multi-path SVGs. VS Code supports image-backed icons and light/high-contrast overrides, while its built-in Seti theme deliberately uses one glyph color per icon and is not a reference for exact multicolor branding. [VS Code file icon theme contract](https://code.visualstudio.com/api/extension-guides/file-icon-theme), [Seti Python glyph definition](https://github.com/microsoft/vscode/blob/28cf0a82a08a5f8e9288d19b4218ec1eb5ee46a6/extensions/theme-seti/icons/vs-seti-icon-theme.json#L1145-L1152)
3. **Fix Slash Command and File Changes at the rendering boundary before adding machinery.** Use a dedicated low-cost command overlay, compositor-friendly entry/exit, and a compact/heavy disclosure split. Do not add virtualization merely because the list can contain 64 rows: `cmdk` itself reports good performance through roughly 2,000–3,000 items. Height measurement is reasonable for small stable content, but not for a large live diff on every animation frame. [`cmdk` performance/virtualization guidance](https://github.com/pacocoursey/cmdk/blob/dd2250ed608443e8f32bafc5fa2d1d07a3746aa3/README.md#L432-L446), [Radix Collapsible measurement](https://github.com/radix-ui/primitives/blob/f7ecd5ab16f5e1e820eb5786a1419a98a2d594ae/packages/react/collapsible/src/collapsible.tsx#L161-L222)
4. **Split Task Center into a paged summary query and lazily loaded detail resources.** Do not delete durable task history to hide a list/query design problem. Use a deterministic cursor, an index-compatible order, aggregate counts once, avoid automatic first-row detail hydration, and fetch artifact versions only when their panel opens. Temporal UI is a relevant open-source precedent: its workflow list uses a page token and its workflow detail has a distinct fetch path. [Temporal UI paginated list source](https://github.com/temporalio/ui/blob/ef33b2553bda6005f3bd116736492f94f09c1c59/src/lib/services/workflow-service.ts#L1121-L1155), [Temporal UI detail fetch](https://github.com/temporalio/ui/blob/ef33b2553bda6005f3bd116736492f94f09c1c59/src/lib/services/workflow-service.ts#L279-L301)
5. **Compile both prompt-cache and reasoning behavior from `provider + endpoint + API style + model`, and treat routing affinity as a separate capability.** DeepSeek direct caching is automatic exact-prefix caching; Alibaba Model Studio Qwen has explicit and implicit modes with interface-specific usage schemas; OpenRouter may change the upstream endpoint and provides sticky routing plus opt-in route metadata. Reasoning is equally provider-specific: Moonshot K3, Alibaba-routed K3, Qwen3.8, DeepSeek V4, GLM, and MiniMax use different fields and value sets. A single `promptCache: boolean`, generic `thinkingBudget`, or model-name branch cannot represent these contracts. [DeepSeek context caching](https://api-docs.deepseek.com/guides/kv_cache), [Alibaba Model Studio context cache](https://help.aliyun.com/en/model-studio/context-cache), [Moonshot reasoning-effort guide](https://platform.kimi.ai/docs/guide/use-reasoning-effort), [Alibaba OpenAI-compatible Chat API](https://help.aliyun.com/en/model-studio/qwen-api-via-openai-chat-completions), [OpenRouter prompt caching and stickiness](https://openrouter.ai/docs/guides/best-practices/prompt-caching)

## 1. Verified Nexa baseline

The following observations refer to the synchronized baseline commit `c7463f12a9c46be848390efe1962a6da17b4b00b`.

- Streaming state records `_launchStartedAt` with `performance.now()` and uses it for launch telemetry, but `StreamState` does not expose a turn lifecycle timing object. [Nexa stream timing](https://github.com/MLGBJDLW/Nexa/blob/c7463f12a9c46be848390efe1962a6da17b4b00b/apps/desktop/src/lib/streamStore.ts#L216-L232), [launch-latency calculation](https://github.com/MLGBJDLW/Nexa/blob/c7463f12a9c46be848390efe1962a6da17b4b00b/apps/desktop/src/lib/streamStore.ts#L356-L380), [public stream state](https://github.com/MLGBJDLW/Nexa/blob/c7463f12a9c46be848390efe1962a6da17b4b00b/apps/desktop/src/lib/streaming/protocol.ts#L107-L145)
- `FileBadgeIconStyle` is exactly one `tone`, one React icon, and one `iconId`; Python resolves to the yellow tone, and `FileBadge` uses that single tone for the badge treatment. [Nexa file visual catalog](https://github.com/MLGBJDLW/Nexa/blob/c7463f12a9c46be848390efe1962a6da17b4b00b/apps/desktop/src/components/ui/fileBadgeCatalog.ts#L82-L112), [Python mapping](https://github.com/MLGBJDLW/Nexa/blob/c7463f12a9c46be848390efe1962a6da17b4b00b/apps/desktop/src/components/ui/fileBadgeCatalog.ts#L140-L150), [badge rendering](https://github.com/MLGBJDLW/Nexa/blob/c7463f12a9c46be848390efe1962a6da17b4b00b/apps/desktop/src/components/ui/FileBadge.tsx#L53-L88)
- Slash search returns at most 64 matches, scrolls the active ref on selection changes, maps every visible match to a button, and renders inside the shared Popover surface. The shared surface has an 18 px backdrop blur plus scale/translate animation. Selecting a command then animates `height: 0` to `height: auto` with a spring. [Nexa slash result path](https://github.com/MLGBJDLW/Nexa/blob/c7463f12a9c46be848390efe1962a6da17b4b00b/apps/desktop/src/components/chat/ChatInput.tsx#L654-L716), [slash Popover and row mapping](https://github.com/MLGBJDLW/Nexa/blob/c7463f12a9c46be848390efe1962a6da17b4b00b/apps/desktop/src/components/chat/ChatInput.tsx#L1345-L1506), [selected-command spring](https://github.com/MLGBJDLW/Nexa/blob/c7463f12a9c46be848390efe1962a6da17b4b00b/apps/desktop/src/components/chat/ChatInput.tsx#L1519-L1552), [shared overlay tokens](https://github.com/MLGBJDLW/Nexa/blob/c7463f12a9c46be848390efe1962a6da17b4b00b/apps/desktop/src/components/ui/overlay/overlayTokens.css#L1-L26)
- Both the File Changes panel and per-file disclosure mount/unmount `FileDiffBody` conditionally while only the chevron has a transform transition. [Nexa file-diff disclosure](https://github.com/MLGBJDLW/Nexa/blob/c7463f12a9c46be848390efe1962a6da17b4b00b/apps/desktop/src/components/chat/FileDiffPreview.tsx#L403-L522), [single-file disclosure](https://github.com/MLGBJDLW/Nexa/blob/c7463f12a9c46be848390efe1962a6da17b4b00b/apps/desktop/src/components/chat/FileDiffPreview.tsx#L562-L630)
- Task Center asks for 80 runs, automatically selects the first run, fetches nine detail resources together, then separately fetches version lists for as many as twelve saved artifacts. [Nexa Task Center initial selection](https://github.com/MLGBJDLW/Nexa/blob/c7463f12a9c46be848390efe1962a6da17b4b00b/apps/desktop/src/pages/TaskCenterPage.tsx#L304-L353), [detail waterfall](https://github.com/MLGBJDLW/Nexa/blob/c7463f12a9c46be848390efe1962a6da17b4b00b/apps/desktop/src/pages/TaskCenterPage.tsx#L372-L421)
- The backing list query uses five correlated count subqueries and orders by `datetime(updated_at)` / `datetime(created_at)`. [Nexa task-run list SQL](https://github.com/MLGBJDLW/Nexa/blob/c7463f12a9c46be848390efe1962a6da17b4b00b/crates/core/src/conversation/mod.rs#L1941-L1968)
- The OpenAI-compatible adapter decides cache behavior from provider/model branches, puts a cache marker on the final converted tool definition for supported branches, and separately marks messages. [Nexa cache capability branch](https://github.com/MLGBJDLW/Nexa/blob/c7463f12a9c46be848390efe1962a6da17b4b00b/crates/core/src/llm/openai.rs#L526-L635), [tool conversion marker](https://github.com/MLGBJDLW/Nexa/blob/c7463f12a9c46be848390efe1962a6da17b4b00b/crates/core/src/llm/openai.rs#L748-L763)
- Usage storage already has `latency_ms` and `provider_raw`, but current model-step call sites supply no latency and often pass normalized usage as the raw fragment. [Nexa usage record contract](https://github.com/MLGBJDLW/Nexa/blob/c7463f12a9c46be848390efe1962a6da17b4b00b/crates/core/src/usage_analytics.rs#L38-L52), [conversation call site](https://github.com/MLGBJDLW/Nexa/blob/c7463f12a9c46be848390efe1962a6da17b4b00b/apps/desktop/src-tauri/src/commands/conversation.rs#L852-L866)

## 2. Turn timing: live display and persistence

### 2.1 Facts that constrain the design

High Resolution Time distinguishes the monotonic clock from the wall clock. The monotonic clock never decreases and is correct for measuring duration, but it exists only within one user-agent execution. The wall clock can move due to system-clock adjustment, but it is the clock for user-visible calendar timestamps and survives process restarts. Background pages may throttle or freeze timers without changing the accuracy of the monotonic clock itself. [W3C clock definitions](https://www.w3.org/TR/hr-time-3/#sec-clocks), [W3C `performance.now()` requirement](https://www.w3.org/TR/hr-time-3/#dom-performance-now)

The HTML standard exposes `visibilityState` and `visibilitychange`, so pausing repaint ticks while hidden and recomputing from timestamps on return is a platform-supported design. The interval count itself must not be treated as elapsed time. [HTML page visibility](https://html.spec.whatwg.org/multipage/interaction.html#page-visibility)

OpenAI Codex models a turn as a lifecycle: `turn/started`, incremental item notifications, then `turn/completed`; command items carry their own `durationMs`. This is useful separation for Nexa: turn wall time, phase time, and tool duration are related but not interchangeable. [Codex turn lifecycle](https://github.com/openai/codex/blob/8e3b5d3e875fd52f1edff75e2f055e4990e866c0/codex-rs/app-server/README.md#L1497-L1509), [Codex command duration](https://github.com/openai/codex/blob/8e3b5d3e875fd52f1edff75e2f055e4990e866c0/codex-rs/app-server/README.md#L1517-L1527)

OpenTelemetry's current GenAI conventions distinguish client time-to-first-chunk, provider/model identity, request/response model, and server address. These are observability dimensions, not the same as user-visible turn duration. [GenAI time-to-first-chunk metric](https://github.com/open-telemetry/semantic-conventions-genai/blob/fe5608e249d64bc5961329a82f8915fe95ced51a/docs/gen-ai/gen-ai-metrics.md#L301-L327), [GenAI span dimensions](https://github.com/open-telemetry/semantic-conventions-genai/blob/fe5608e249d64bc5961329a82f8915fe95ced51a/model/gen-ai/spans.yaml#L17-L45)

GitHub Primer recommends avoiding a flashing loading state below one second, using an indeterminate state for roughly one to three seconds, and showing more concrete progress for longer waits. That supports the proposal to show state immediately but defer the numeric elapsed label for a short grace period. [Primer loading guidance](https://primer.style/product/ui-patterns/loading/#adapting-to-different-wait-times)

### 2.2 Recommended Nexa contract

Persist facts, not a ticking counter:

```ts
interface TurnTiming {
  clientStartedAtEpochMs: number;
  runtimeStartedAtEpochMs?: number;
  firstEventAtEpochMs?: number;
  firstVisibleOutputAtEpochMs?: number;
  finishedAtEpochMs?: number;
  terminalStatus?: 'completed' | 'failed' | 'cancelled' | 'timed_out';
  currentPhase?: TurnPhase;
  phaseStartedAtEpochMs?: number;
}
```

Within the live WebView, keep a non-persisted monotonic anchor such as `{epochAtStart, monotonicAtStart}` for smooth duration calculation. On restart, reconstruct elapsed time only from persisted epoch timestamps. Set the terminal timestamp for every terminal path, including cancellation, timeout, and failure.

Render policy:

- `ChatRunOverview`: show the phase immediately and numeric elapsed time after about three seconds.
- Completed assistant turn footer: fixed total duration; no interval remains alive.
- Task Center summary: total duration/state only; phase breakdown belongs to detail.
- Tool cards: keep their own `durationMs`; do not repeat the turn clock inside each card.
- A small `TurnElapsedBadge` owns its local tick and visibility subscription. The stream store contains timestamp facts only. React requires external-store snapshots to be cached and stable; injecting a new elapsed value into the shared stream snapshot every second would defeat that stability and broaden rerenders. [React `useSyncExternalStore` snapshot contract](https://react.dev/reference/react/useSyncExternalStore#im-getting-an-error-the-result-of-getsnapshot-should-be-cached)

### 2.3 What not to copy

- Do not persist `performance.now()` as an absolute time or compare it across app executions; the W3C contract explicitly limits that monotonic clock to a user-agent execution.
- Do not derive duration by counting interval callbacks. Hidden/throttled pages make callback count diverge from wall time.
- Do not infer a full timing contract from Codex's current `turn/started` / `turn/completed` objects; they establish lifecycle boundaries, not Nexa's required persisted timestamps or pause semantics.
- Do not use one field named `latency` for turn wall time, provider time-to-first-chunk, and tool duration. They need distinct names and measurement points.

## 3. File Badge brand and multicolor visuals

### 3.1 Facts that constrain the design

VS Code file icon themes map file names, extensions, and language IDs to icon definitions. An icon may be an SVG/PNG path or a font glyph; associations can be overridden for light and high-contrast themes. This validates an asset-backed branch in Nexa's file visual catalog and explicit theme variants. [VS Code file icon theme guide](https://code.visualstudio.com/api/extension-guides/file-icon-theme)

VS Code's built-in Seti theme is a useful taxonomy example but not a faithful-brand example. Its Python and TypeScript icons are font glyphs with one `fontColor` each. Nexa should borrow deterministic mapping/fallback behavior, not the monochrome visual limitation. [Seti Python definition](https://github.com/microsoft/vscode/blob/28cf0a82a08a5f8e9288d19b4218ec1eb5ee46a6/extensions/theme-seti/icons/vs-seti-icon-theme.json#L1145-L1152), [Seti TypeScript definition](https://github.com/microsoft/vscode/blob/28cf0a82a08a5f8e9288d19b4218ec1eb5ee46a6/extensions/theme-seti/icons/vs-seti-icon-theme.json#L1404-L1418)

Simple Icons' current Python asset is a single SVG path, so applying CSS `color` can only make that path monochrome. Its disclaimer also says the package's CC0 status does not imply every included brand icon is CC0 and tells users to check each icon's license and brand guidelines. [Simple Icons Python SVG](https://github.com/simple-icons/simple-icons/blob/34c22501f9ac9f22b12f825677ccbab1fb22e14b/icons/python.svg#L1), [Simple Icons legal disclaimer](https://github.com/simple-icons/simple-icons/blob/34c22501f9ac9f22b12f825677ccbab1fb22e14b/DISCLAIMER.md#L3-L34)

### 3.2 Recommended Nexa model

Separate icon rendering from badge shell tone:

```ts
type FileIconTreatment = 'mono' | 'brand-accent' | 'brand-svg';

interface FileBadgeVisual {
  iconId: string;
  treatment: FileIconTreatment;
  Icon?: React.ElementType;
  assetId?: BrandAssetId;
  shellTone: FileBadgeTone;
  primary?: BrandColorToken;
  secondary?: BrandColorToken;
  highContrastFallbackIconId?: string;
}
```

Recommended rollout:

- Keep the badge background, border, and filename text in Nexa's semantic theme tokens.
- Use one audited brand color on the icon for the broad catalog.
- Add exact multi-path SVGs only for a small, high-frequency allow-list where the second color materially improves recognition.
- Provide light, dark, and forced/high-contrast variants. High contrast may intentionally fall back to a one-color silhouette.
- Preserve the existing generic fallback and fixed icon box so visual upgrades never change badge geometry.
- Record asset source, upstream version, license, trademark/brand-guideline URL, and review date beside each packaged custom asset.

### 3.3 What not to copy

- Do not recolor the entire badge to a saturated brand palette. File badges appear in dense chat/tool surfaces; exact logo color is not a license to reduce text contrast or create visual noise.
- Do not promise exact two-color Python from the current `SiPython` component; its upstream SVG is one path.
- Do not copy arbitrary SVGs found through image search. Even Simple Icons requires per-icon license and brand-guideline review.
- Do not copy VS Code Seti's icon font architecture just to get more colors. Nexa already ships React/SVG icons, and an icon font adds a new asset/loading/fallback system while Seti still supplies one color per glyph.

## 4. Slash Command and File Changes motion

### 4.1 Facts that constrain the design

`cmdk` does not virtualize but reports good performance with about 2,000–3,000 items; it recommends `shouldFilter={false}` when consumers need manual filtering or their own virtualizer. This is strong evidence that 64 simple rows alone do not justify an immediate virtualization dependency. Nexa should first measure filtering, row reconciliation, scroll calls, Popover paint, and the auto-height spring. [`cmdk` FAQ](https://github.com/pacocoursey/cmdk/blob/dd2250ed608443e8f32bafc5fa2d1d07a3746aa3/README.md#L432-L446)

Radix Collapsible mounts content through `Presence`, measures the full dimensions, exposes `--radix-collapsible-content-height`, and hides/unmounts after closing. That is a proven compact disclosure pattern, but the measurement cost grows with content and does not make height animation free. [Radix Collapsible content lifecycle](https://github.com/radix-ui/primitives/blob/f7ecd5ab16f5e1e820eb5786a1419a98a2d594ae/packages/react/collapsible/src/collapsible.tsx#L124-L140), [Radix dimension measurement](https://github.com/radix-ui/primitives/blob/f7ecd5ab16f5e1e820eb5786a1419a98a2d594ae/packages/react/collapsible/src/collapsible.tsx#L161-L222)

Chrome's first-party animation guidance recommends avoiding animated properties that trigger layout or paint and favors `transform` and `opacity` for smooth animation. [web.dev high-performance animation guide](https://web.dev/articles/animations-guide#avoid-properties-that-trigger-layout-or-paint)

Paint containment can isolate work, but it also clips descendant ink/scrollable overflow and creates a containing block and stacking context. Therefore `contain: layout paint` belongs on a bounded list/diff viewport only after visual testing, not blindly on the outer Popover whose shadows and positioned content may rely on overflow. [CSS Containment paint effects](https://www.w3.org/TR/css-contain-2/#containment-paint)

The platform's reduced-motion preference requests removal or replacement of non-essential motion. GitHub Primer likewise recommends subtle motion and wrapping non-essential transitions in the no-preference branch. [W3C `prefers-reduced-motion`](https://www.w3.org/TR/mediaqueries-5/#prefers-reduced-motion), [Primer motion guidance](https://primer.style/accessibility/design-guidance/motion-and-animation/#support-reduced-motion)

Long Tasks and Long Animation Frames use a 50 ms threshold. LoAF adds render and style/layout timing, which is more diagnostic for a janky disclosure than FPS guessed from screenshots. [W3C Long Tasks](https://www.w3.org/TR/longtasks-1/), [Chrome Long Animation Frames diagnostics](https://developer.chrome.com/docs/web-platform/long-animation-frames)

### 4.2 Recommended Nexa motion system

Use a shared vocabulary, but keep workload-specific implementations:

```css
--motion-instant: 80ms;
--motion-fast: 120ms;
--motion-standard: 160ms;
--motion-slow: 220ms;
--ease-out-standard: cubic-bezier(.2, .8, .2, 1);
--ease-out-emphasized: cubic-bezier(.16, 1, .3, 1);
```

These values are proposed Nexa tokens, not claims from an upstream standard.

**Slash Command**

- Give the command menu a dedicated surface modifier that removes the 18 px backdrop blur and scale animation. Use short opacity plus 2–4 px `translateY`.
- Precompute normalized search fields when the command catalog changes, not on each keystroke.
- Memoize rows and update only the active rows whose selected state changed.
- Do not call `scrollIntoView` when the active row is already within the scroll viewport.
- A visible cap such as 12–20 rows may improve scanning and DOM work, but keep the full result count and keyboard access. This is a Nexa product choice, not a requirement from `cmdk`.
- Add a virtualizer only after measured growth well beyond the current catalog or after profiling proves row mounting is the bottleneck.
- Replace the selected-command `height: auto` spring with a fixed/min-height slot or short grid/clip disclosure; use opacity/translation for the content.

**File Changes**

- Compact content: a measured disclosure (Radix-style height variable or CSS grid `0fr` to `1fr`) is acceptable for a bounded, stable number of lines.
- Heavy/live content: constrain the body to a maximum viewport with its own scroll; animate only the outer opacity/clip/translation. Mount expensive diff content at the start or next frame, then stop height animation while live lines append.
- Preserve disclosure semantics (`aria-expanded`, stable trigger focus) and provide an immediate/reduced-motion path.

**Measurement**

- Record slash input timestamp, React commit, and next paint.
- Observe `longtask`; where supported, observe `long-animation-frame` and its style/layout duration.
- Test keyboard repeat, theme blur enabled/disabled, 64 results, a 500-line diff, and live diff appends on the supported Windows WebView runtime.
- A useful release gate is zero interaction-attributable frames above 50 ms in the controlled scenario plus no regression in P95 keystroke-to-paint from the current baseline. A universal “always below 16 ms” promise is not source-backed and should be treated as an aspiration, not the only pass/fail rule.

### 4.3 What not to copy

- Do not add `cmdk` or a virtualizer solely because it is popular; Nexa's current menu is custom and the upstream library says its own unvirtualized list scales far past 64 rows.
- Do not apply Radix-style measured height to an unbounded live diff. Its source proves it reads full dimensions; it does not remove layout work.
- Do not place `contain: paint` on the entire overlay without testing clipping, portal positioning, focus rings, and shadows.
- Do not turn reduced motion into loss of state feedback. Remove movement while keeping an immediate opacity/state change and semantic status.

## 5. Task Center summary pagination and lazy detail

### 5.1 Facts that constrain the design

Temporal UI, a production open-source workflow/task surface, exposes a paginated list function that passes `pageSize` and `nextPageToken`, while `fetchWorkflow` retrieves an individual workflow from a separate route. The relevant pattern is separation of collection and detail; Nexa should not copy Temporal's wire token or default page size mechanically. [Temporal paginated workflows](https://github.com/temporalio/ui/blob/ef33b2553bda6005f3bd116736492f94f09c1c59/src/lib/services/workflow-service.ts#L1121-L1155), [Temporal detail fetch](https://github.com/temporalio/ui/blob/ef33b2553bda6005f3bd116736492f94f09c1c59/src/lib/services/workflow-service.ts#L279-L301)

OpenAI Codex now also separates live resume state from paged durable turns: clients may request `excludeTurns: true` and page stored history through a cursor API. This supports the same architectural separation for Nexa Task Center summaries. [Codex paginated turn contract](https://github.com/openai/codex/blob/8e3b5d3e875fd52f1edff75e2f055e4990e866c0/codex-rs/app-server/README.md#L331-L337), [Codex turn-list cursor](https://github.com/openai/codex/blob/8e3b5d3e875fd52f1edff75e2f055e4990e866c0/codex-rs/app-server/README.md#L535-L555)

SQLite documents that deep `OFFSET` work grows with the offset because it computes and discards preceding rows. It demonstrates a scrolling-window query using row-value comparison instead. [SQLite scrolling-window queries](https://www.sqlite.org/rowvalue.html#scrolling_window_queries)

SQLite can satisfy `ORDER BY` from an index; otherwise it may build a temporary B-tree. Expression indexes are considered only when the indexed expression appears exactly as written in the query. Therefore an index on raw `updated_at` does not automatically prove that `ORDER BY datetime(updated_at)` is index-backed. [SQLite query planner and sorting](https://www.sqlite.org/queryplanner.html#sorting), [SQLite expression-index matching](https://www.sqlite.org/expridx.html)

`EXPLAIN QUERY PLAN` distinguishes `SCAN`, `SEARCH ... USING INDEX`, correlated scalar subqueries, and temporary sort B-trees. It is the correct acceptance evidence for the new list query, though its output format must not become application logic. [SQLite `EXPLAIN QUERY PLAN`](https://sqlite.org/eqp.html)

### 5.2 Recommended Nexa API and storage path

Use an opaque API cursor encoding the deterministic sort tuple:

```ts
interface AgentTaskRunSummaryPage {
  items: AgentTaskRunSummary[];
  nextCursor: string | null;
}

listAgentTaskRunSummaries({
  limit: 25,
  cursor,
  status,
  projectId,
}): Promise<AgentTaskRunSummaryPage>
```

For the current ISO-8601 UTC text columns, the backing query can use a stable tuple such as `(updated_at, created_at, id)` in descending order. The cursor must include all tie-breakers. Treat it as opaque above the Rust/SQL boundary so the representation can change.

List data should contain only fields required to paint the row: identity, title, state/phase, timestamps, preview, aggregate counts, and a few artifact kinds. Do not parse or return complete plan/artifact payloads on the collection path.

Recommended query changes:

- Remove `datetime()` from the sort only after verifying that all stored timestamps use one lexicographically sortable canonical format and adding a migration/test for legacy rows.
- Add a recency index aligned to filter and order; use a status/project prefix only for query shapes that actually filter by it.
- Replace per-row correlated counts with grouped CTE/subqueries joined once, or maintain counters transactionally on the run row if profiling justifies denormalization.
- Run `EXPLAIN QUERY PLAN` against empty, realistic, and 10,000-run fixtures; require no temporary B-tree for the main recency order and no correlated count subquery per output row.

Frontend loading policy:

- Opening Task Center loads summary page plus only globally necessary permission/scheduler state.
- Do not auto-select the first run unless a `runId` route, restored user selection, or explicit product rule asks for it.
- Selecting a run loads a small overview. Timeline, execution/investigation graph, checkpoints, memories, artifacts, and automation events load when their panel becomes visible.
- Artifact version history loads only when the user opens or edits that artifact, not for the first twelve artifacts on every selection.
- Give each selection a request generation or cancellation token so late responses cannot overwrite the new run.
- Cache completed-run detail longer than active-run detail and update active rows from existing events rather than reloading every panel.

Retention is a different policy. Keep durable summaries and user-owned artifacts by default; if storage needs limits, compact bulky events/versions independently and protect pinned, active, checkpointed, or user-edited tasks.

### 5.3 What not to copy

- Do not copy Temporal's default page size or opaque server token; copy the collection/detail separation and pagination behavior.
- Do not use an `updated_at`-only cursor: equal timestamps can skip or duplicate rows.
- Do not remove `datetime()` and assume correctness without validating the complete legacy timestamp corpus.
- Do not delete history or lower `80` to `25` while leaving automatic selection and the detail waterfall intact; that reduces symptoms but preserves the architecture problem.
- Do not assert that a new index is used merely because it exists. Capture the actual query plan in a test or benchmark artifact.

## 6. Provider/endpoint/model prompt-cache profiles and observability

### 6.1 Provider facts are materially different

#### Direct DeepSeek API

DeepSeek context caching is enabled by default. Hits require a fully matching persisted prefix unit; the response reports `prompt_cache_hit_tokens` and `prompt_cache_miss_tokens`. Cache construction takes seconds, cleanup usually occurs after hours to days of non-use, and the service is best-effort rather than guaranteed. [DeepSeek context caching](https://api-docs.deepseek.com/guides/kv_cache)

Nexa implication: keep deterministic, append-only stable prefixes and parse the two native usage fields. Send no explicit marker. Cache construction taking seconds does **not** justify a fixed sleep before every follow-up; the official contract gives no fixed ready time.

#### Alibaba Cloud Model Studio Qwen

Alibaba documents two mutually exclusive modes. Explicit cache uses `cache_control: {"type":"ephemeral"}`, needs at least 1,024 tokens, allows up to four markers, looks backward through at most 20 content blocks, is created only after the model finishes, and has a five-minute TTL that refreshes on a hit. Implicit cache is automatic, has a lower documented minimum, no guaranteed hit probability, and a provider-managed lifetime. [Alibaba explicit and implicit cache rules](https://help.aliyun.com/en/model-studio/context-cache)

Tool definitions are included in the system prefix, but cannot be cached independently: markers attached to tool definitions are ignored, and tool order, JSON field order, and field structure must remain identical. Markers may be placed on system, user, assistant, and tool-result message content. Alibaba also recommends merging parallel tool results to stay inside the 20-block lookup window. [Alibaba cacheable content and tool rules](https://help.aliyun.com/en/model-studio/context-cache)

Usage schema varies by API style and, in some DashScope cases, region/model: OpenAI-compatible responses use `usage.prompt_tokens_details.cached_tokens`; Anthropic-compatible responses use `usage.cache_read_input_tokens`; other documented shapes include `usage.cached_tokens` and cache-creation fields. [Alibaba interface-specific usage examples](https://help.aliyun.com/en/model-studio/context-cache)

Nexa implication: remove Qwen cache markers from `tools`; compile message-content markers from an endpoint/model allow-list and API-style encoder. Do not infer explicit-cache support from the substring `qwen` alone.

#### OpenRouter

OpenRouter's prompt caching page distinguishes automatic and explicit provider caches. Its support list is model/endpoint-specific; for example, Alibaba explicit caching is listed only for selected aliases and excludes listed snapshot endpoints. OpenRouter uses provider sticky routing; a top-level `session_id` (maximum 256 characters) activates stickiness after any successful request and is useful when an agent's opening messages change. A manual `provider.order` overrides sticky routing. [OpenRouter prompt caching and provider stickiness](https://openrouter.ai/docs/guides/best-practices/prompt-caching)

Every response includes normalized usage with `cached_tokens` and, where applicable, `cache_write_tokens`. Opting into `X-OpenRouter-Metadata: enabled` adds selected provider/model, attempts, fallback strategy, and pipeline transformations. However, OpenRouter intentionally removes `openrouter_metadata` from response-cache hits, so the field is not a guaranteed per-request source of upstream identity. A generation record can later expose `provider_name`, native cached tokens, latency, generation time, upstream ID, cost, and session ID. [OpenRouter usage accounting](https://openrouter.ai/docs/cookbook/administration/usage-accounting), [router metadata and cache-hit limitation](https://openrouter.ai/docs/guides/features/router-metadata), [generation metadata](https://openrouter.ai/docs/api/api-reference/generations/get-generation)

Nexa implication: use a stable privacy-preserving session identifier for a conversation when OpenRouter is the configured endpoint, unless the user explicitly supplies an affinity key. Record requested route, successful route metadata when available, and generation ID; classify missing route metadata on a replay as “not reported”, not route drift.

### 6.2 Reasoning controls, Kimi routing, and Qwen3.8 availability

The facts in this subsection were rechecked against official provider documentation on **2026-08-04**. The precise endpoint is part of the contract: an identical-looking model name on Moonshot direct, Alibaba-hosted Model Studio, Alibaba's third-party route, standard pay-as-you-go QwenCloud, and Token Plan must not inherit another endpoint's reasoning encoder or model allow-list.

#### Terms that must remain distinct

- `reasoning_effort` is a categorical control. Its accepted values, aliases, default, and wire location vary by model and API style.
- `thinking_budget` is a numeric token ceiling supported only by documented models/interfaces. It is not a universal translation of `max`, and it may be mutually exclusive with `reasoning_effort`.
- `enable_thinking` is a provider-specific boolean switch. It cannot encode Moonshot's `thinking` object or MiniMax M3's adaptive mode.
- `thinking` is an object with a model-specific shape. Moonshot K2.6 uses `{type, keep}`; K2.5 uses `{type}`; Alibaba-routed MiniMax M3 uses `{type: "adaptive" | "disabled"}`.
- `auto` is not a documented K3 or Qwen3.8 reasoning level. In the cited Kimi contracts it appears under `tool_choice`; it must not be mapped to a thinking budget or effort. [Moonshot K3 model contract](https://platform.kimi.ai/docs/api/models-overview), [Alibaba MiniMax-by-MiniMax API](https://help.aliyun.com/en/model-studio/minimax-api-by-minimax), [Alibaba OpenAI-compatible Chat API](https://help.aliyun.com/en/model-studio/qwen-api-via-openai-chat-completions)

#### Moonshot direct API

Moonshot direct uses `https://api.moonshot.ai/v1`. Its current reasoning contracts are:

| Direct model | Thinking mode | Request control | Budget/preservation facts |
| --- | --- | --- | --- |
| `kimi-k3` | Always on | Top-level `reasoning_effort`: `low`, `high`, or `max`; default `max` | No separate `thinking_budget` or `thinking` switch is documented. Multi-turn/tool replay must preserve the complete assistant message, including `reasoning_content` and `tool_calls`. |
| `kimi-k2.7-code`, `kimi-k2.7-code-highspeed` | Always on | Do not expose effort; do not use a disable switch | Preserved Thinking is always on; replay the complete assistant message. |
| `kimi-k2.6` | Hybrid | `thinking.type`: `enabled` by default or `disabled`; `thinking.keep`: `null` by default or `all` | No `reasoning_effort` or separate `thinking_budget` is documented. |
| `kimi-k2.5` | Hybrid | `thinking.type`: `enabled` by default or `disabled` | No `thinking.keep`, `reasoning_effort`, or separate `thinking_budget` is documented. Moonshot says it stops serving this model on 2026-08-31. |

For K2 thinking models, Moonshot defines `max_tokens` as the combined ceiling for `reasoning_content` plus final `content`; that is not a dedicated reasoning budget. K3 has a 1,048,576-token context and automatic prefix-cache eligibility once the preceding prompt exceeds 256 tokens. K2.7, K2.6, and K2.5 are documented at 262,144 context. Moonshot also marks older K2-series aliases and `kimi-latest` as discontinued, so they should not remain selectable merely because an old catalog entry exists. [Moonshot reasoning-effort guide](https://platform.kimi.ai/docs/guide/use-reasoning-effort), [Moonshot thinking-model guide](https://platform.kimi.ai/docs/guide/use-thinking-models), [Moonshot model overview](https://platform.kimi.ai/docs/api/models-overview), [Moonshot models and lifecycle](https://platform.kimi.ai/docs/models), [Moonshot K3 quickstart](https://platform.kimi.ai/docs/guide/kimi-k3-quickstart)

#### Alibaba Model Studio Kimi routes

Alibaba exposes two different Kimi families, and their identifiers are intentionally not interchangeable:

| Alibaba route | Exact examples | Region/endpoint scope | Reasoning contract |
| --- | --- | --- | --- |
| Models supplied by Moonshot AI | `kimi/kimi-k3`, `kimi/kimi-k2.7-code-highspeed`, `kimi/kimi-k2.7-code`, `kimi/kimi-k2.6`, `kimi/kimi-k2.5` | China (Beijing), standard Model Studio workspace API key and Beijing OpenAI-compatible endpoint | K3 is always-thinking and accepts only top-level `reasoning_effort: "max"`; it has no documented numeric budget. K2.7 is always-thinking. K2.6/K2.5 use `enable_thinking`; no routed K2.x numeric budget is documented by the dedicated page. |
| Alibaba-hosted Kimi | `kimi-k2.7-code`, `kimi-k2.6`, `kimi-k2.5` | Model Studio regions listed by the dedicated model page: Beijing, Singapore, Tokyo, Virginia, and Frankfurt | K2.7 is always-thinking. K2.6/K2.5 use `enable_thinking`, default off on the dedicated hosted-Kimi page. The general Chat/deep-thinking references also support `thinking_budget`; do not add a generic effort control. |

The current Chinese Moonshot-route page describes K2.6/K2.5 as “thinking enabled by default” in one section, but its parameter section says omission of `enable_thinking` selects non-thinking output. This is an internal official-document conflict; Nexa should always send an explicit boolean for those routed hybrid models rather than rely on the omitted-field default. For Alibaba-hosted Kimi, both the general Chat reference and the deep-thinking guide explicitly include Kimi “supplied by Alibaba Cloud” in `thinking_budget` support. They say the omitted budget defaults to the model's maximum chain-of-thought length, but direct readers to the console model page for that model-specific value; the dedicated hosted-Kimi page does not publish a static bound. Nexa can therefore encode budget support while keeping min/max/default unset until exact model metadata is available, rather than inventing a provider-wide 10,000-token default. [Alibaba Moonshot-supplied Kimi API](https://help.aliyun.com/zh/model-studio/kimi-api-by-moonshot-ai), [Alibaba-hosted Kimi API](https://help.aliyun.com/en/model-studio/kimi-api), [Alibaba deep-thinking guide](https://help.aliyun.com/en/model-studio/deep-thinking), [Alibaba OpenAI-compatible Chat API](https://help.aliyun.com/en/model-studio/qwen-api-via-openai-chat-completions)

The Moonshot-supplied K3 model card reports text, image, and video input; text output; function calling, structured output, prefix completion, and context caching; and a 1,048,576-token context/output ceiling. This routed capability record must be scoped to `provider=AlibabaModelStudio`, the Beijing endpoint, and model ID `kimi/kimi-k3`; it is not evidence that direct Moonshot or another Alibaba region exposes the same wire contract. [Alibaba Kimi K3 model card](https://help.aliyun.com/zh/model-studio/kimi-k3)

#### Qwen3.8 Max formal release and API-style scope

The formal model ID is **`qwen3.8-max`**. QwenCloud's official model page reports text/image/video input, text output, reasoning, prefix completion, function calling, context caching, structured output, batch processing, and web search. It displays a 1M context, 991K maximum non-thinking input, 983K maximum thinking input, 131K maximum output, and 262K maximum reasoning length. These are the provider's displayed rounded units and should not be silently expanded into invented exact integers. QwenCloud's Responses implementation additionally advertises built-in `code_interpreter`, `web_extractor`, `web_search`, `t2i_search`, and `i2i_search` tools. [QwenCloud Qwen3.8 Max model page](https://www.qwencloud.com/models/qwen3.8-max)

Alibaba Model Studio's current model list exposes the formal model in Beijing, Singapore, Tokyo, Frankfurt, and US Virginia. Its standard regional endpoint families are:

| Region | Standard OpenAI-compatible endpoint |
| --- | --- |
| China (Beijing) | `https://{WorkspaceId}.cn-beijing.maas.aliyuncs.com/compatible-mode/v1` |
| Singapore | `https://{WorkspaceId}.ap-southeast-1.maas.aliyuncs.com/compatible-mode/v1` |
| Japan (Tokyo) | `https://{WorkspaceId}.ap-northeast-1.maas.aliyuncs.com/compatible-mode/v1` |
| Germany (Frankfurt) | `https://{WorkspaceId}.eu-central-1.maas.aliyuncs.com/compatible-mode/v1` |
| US (Virginia) | `https://dashscope-us.aliyuncs.com/compatible-mode/v1` |

The same official page lists regional DashScope and Anthropic-compatible endpoint variants. Nexa should store them as distinct endpoint/API-style records, not aliases of one base URL. [Alibaba Model Studio model and endpoint list](https://help.aliyun.com/en/model-studio/models)

For OpenAI-compatible **Chat Completions**, both `qwen3.8-max` and `qwen3.8-max-preview` support native effort values `low`, `medium`, and `xhigh`, with `xhigh` as the documented default. Compatibility aliases map `minimal` to `low`, `high` and `max` to `xhigh`, and `none` to `enable_thinking: false`. Effort and numeric `thinking_budget` are mutually exclusive. The documented mappings are:

| Effort | Effective budget | Numeric budget mapped back to effort |
| --- | ---: | --- |
| `low` | 4,096 | 0–4,096 |
| `medium` | 16,384 | 4,097–16,384 |
| `xhigh` (`high`/`max` aliases) | 262,144 | 16,385–262,144 |

The Chat reference separately says that omitting both fields defaults `thinking_budget` to 131,072 while the model's default effort is `xhigh`. Those two statements describe an official defaulting subtlety; Nexa should preserve “unset” rather than manufacture a client-side 131,072-to-effort conversion. It should also preserve `reasoning_content` in history because `preserve_thinking` defaults true for the formal and preview variants. [Alibaba OpenAI-compatible Chat API](https://help.aliyun.com/en/model-studio/qwen-api-via-openai-chat-completions)

For OpenAI-compatible **Responses**, effort moves to `reasoning.effort`; the reference accepts `none`, `minimal`, `low`, `medium`, `high`, `xhigh`, and `max`, and it does not define `thinking_budget`. The Responses supported-model table currently lists Qwen3.8 Max only for Beijing, even though the broader model marketplace lists the formal model in five regions. This is scope rather than permission to extrapolate: Nexa should advertise a capability only when the exact `{region, api_style, model}` combination is present in the relevant API reference, or after a live model-list/probe test. [Alibaba OpenAI-compatible Responses API](https://help.aliyun.com/en/model-studio/qwen-api-via-openai-responses), [Alibaba Model Studio model list](https://help.aliyun.com/en/model-studio/models)

#### Token Plan is a separate provider endpoint, credential, and allow-list

Token Plan and standard pay-as-you-go Model Studio/QwenCloud are not interchangeable:

| Product | OpenAI-compatible endpoint | Credential scope | Current official model-list boundary |
| --- | --- | --- | --- |
| China standard Model Studio | Regional workspace endpoint, for example `https://{WorkspaceId}.cn-beijing.maas.aliyuncs.com/compatible-mode/v1` | Workspace/region-scoped standard API key (`sk-ws...`; legacy standard keys remain documented) | Standard regional Model Studio matrix |
| China Token Plan | `https://token-plan.cn-beijing.maas.aliyuncs.com/compatible-mode/v1` | Dedicated subscription key beginning `sk-sp`; not interchangeable with a standard key | Beijing only. Personal currently lists formal `qwen3.8-max` and preview; Team lists those plus a broader set including Kimi K2.7/K2.6/K2.5, but not K3. |
| Global standard QwenCloud | `https://dashscope-intl.aliyuncs.com/compatible-mode/v1` | Standard QwenCloud pay-as-you-go key | Global pay-as-you-go model matrix |
| Global Token Plan | `https://token-plan.ap-southeast-1.maas.aliyuncs.com/compatible-mode/v1` | Dedicated subscription key beginning `sk-sp` | Singapore/Global deployment only. Both Personal and Team exact allow-lists currently include formal `qwen3.8-max` and preview; Team also includes Kimi K2.7/K2.6/K2.5, but not K3. |

As accessed on 2026-08-04, all four current Personal/Team overviews—China Alibaba Cloud and global QwenCloud—list the formal `qwen3.8-max` by exact string. That availability is still endpoint-specific: it does not make a Token Plan key valid against the standard pay-as-you-go endpoint, nor does it add K3 to the Team plan's Moonshot matrix. Nexa should retain independent allow-lists and refresh them from official docs or a live model-list response. Token Plan's Anthropic-compatible URL is likewise a separate `/apps/anthropic` API style. The subscription overviews also restrict use to interactive coding/agent tools rather than automated scripts or application backends, which is a product entitlement Nexa must surface rather than treating the endpoint as generic pay-as-you-go. [Alibaba Token Plan Personal quickstart](https://help.aliyun.com/en/model-studio/token-plan-personal-quick-start), [Alibaba Token Plan Personal overview](https://help.aliyun.com/en/model-studio/token-plan-personal-overview), [Alibaba Token Plan Team overview](https://help.aliyun.com/en/model-studio/token-plan-team-overview), [Alibaba standard API-key guide](https://help.aliyun.com/en/model-studio/get-api-key), [QwenCloud API-key guide](https://docs.qwencloud.com/api-reference/preparation/api-key), [QwenCloud Token Plan Personal quickstart](https://docs.qwencloud.com/token-plan/personal/token-plan-personal-quickstart), [QwenCloud Token Plan Personal overview](https://docs.qwencloud.com/token-plan/personal/token-plan-personal-overview), [QwenCloud Token Plan Team quickstart](https://docs.qwencloud.com/token-plan/team/token-plan-team-quickstart), [QwenCloud Token Plan Team overview](https://docs.qwencloud.com/token-plan/team/token-plan-team-overview)

#### Provider-aware reasoning matrix Nexa can implement

Resolve the following profiles by exact provider, canonical endpoint, API style, and model ID before request encoding:

| Resolved profile | Mode | Wire field and accepted values | Numeric budget | Replay rule |
| --- | --- | --- | --- | --- |
| Moonshot direct Chat / `kimi-k3` | Always on | `reasoning_effort`: `low`, `high`, `max` (default `max`) | Unsupported | Replay complete assistant message |
| Moonshot direct Chat / K2.7 code family | Always on | None | Unsupported | Preserved Thinking always on |
| Moonshot direct Chat / `kimi-k2.6` | Hybrid | `thinking.type`; optional `thinking.keep` | Unsupported | Preserve according to `keep` |
| Moonshot direct Chat / `kimi-k2.5` | Hybrid | `thinking.type` | Unsupported | No `keep`; flag sunset |
| Alibaba Beijing Chat / `kimi/kimi-k3` | Always on | `reasoning_effort`: `max` only | Unsupported | Preserve reasoning history |
| Alibaba Chat / routed Moonshot K2.x | Always/hybrid by exact model | `enable_thinking` for hybrid models; none for always-on | Unsupported by dedicated route docs | Preserve where model card says supported |
| Alibaba Chat / hosted K2.x | Always/hybrid by exact model | `enable_thinking` for hybrid models | Supported; obtain per-model bounds/default from exact model metadata rather than a provider-wide default | Model-specific |
| Alibaba/QwenCloud/Token Plan Chat / Qwen3.8 | Hybrid for formal; preview is thinking-only | Top-level `reasoning_effort` with aliases above, or `enable_thinking: false` where supported | Supported, mutually exclusive with effort | Preserve `reasoning_content` |
| Alibaba Responses / Qwen3.8 | API-reference scoped | Nested `reasoning.effort` | Unsupported by this API reference | Responses item replay |
| Alibaba Chat / DeepSeek V4 | Hybrid | `enable_thinking`; `reasoning_effort` accepts documented effort values, with lower aliases behaving as `high` and `xhigh` as `max` | Do not inherit Qwen budget | Provider response contract |
| Alibaba Chat / GLM 5.x | Hybrid | `enable_thinking`; use the exact dedicated-model effort set | Do not inherit Qwen budget | Provider response contract |
| Alibaba Chat / `MiniMax/MiniMax-M3` | Adaptive or disabled | `thinking: {"type":"adaptive"}` or `{"type":"disabled"}` | Unsupported | Provider response contract |

For DeepSeek V4, Alibaba's dedicated page says thinking is enabled by default and documents `low`, `medium`, `high`, `xhigh`, and `max` with alias-like behavior. The general English deep-thinking page says the opposite default in one table. Prefer the exact-model page and explicitly send `enable_thinking` when the user changes mode. For GLM, the dedicated page gives GLM 5.2 the broadest set (`none`, `minimal`, `low`, `medium`, `high`, `xhigh`, `max`) and narrower sets to GLM 5.1/5; do not collapse those to the generic Chat page's high/max summary. MiniMax M3 uses a `thinking` object, not either convention. [Alibaba DeepSeek API](https://help.aliyun.com/en/model-studio/deepseek-api), [Alibaba deep-thinking guide](https://help.aliyun.com/en/model-studio/deep-thinking), [Alibaba GLM API](https://help.aliyun.com/en/model-studio/glm), [Alibaba MiniMax-by-MiniMax API](https://help.aliyun.com/en/model-studio/minimax-api-by-minimax)

The current shared catalog can express effort labels and one generic numeric budget, but not wire location, object shape, aliases, exclusivity, preservation, lifecycle, endpoint/API-style allow-lists, or conflicting-doc confidence. The current OpenAI-compatible adapter also groups Qwen, Alibaba Model Studio, and SiliconFlow into one Qwen branch that emits `enable_thinking`/`thinking_budget`; that would mis-encode Alibaba-routed K3 and MiniMax M3. [Nexa reasoning schema](https://github.com/MLGBJDLW/Nexa/blob/c7463f12a9c46be848390efe1962a6da17b4b00b/shared/model-catalog.schema.json#L47-L65), [Nexa OpenAI-compatible reasoning encoder](https://github.com/MLGBJDLW/Nexa/blob/c7463f12a9c46be848390efe1962a6da17b4b00b/crates/core/src/llm/openai.rs#L823-L958), [Nexa Moonshot catalog](https://github.com/MLGBJDLW/Nexa/blob/c7463f12a9c46be848390efe1962a6da17b4b00b/shared/provider-presets.json#L7304-L7333), [Nexa Token Plan and Alibaba catalog](https://github.com/MLGBJDLW/Nexa/blob/c7463f12a9c46be848390efe1962a6da17b4b00b/shared/provider-presets.json#L7433-L7631)

Add an explicit reasoning profile beside the cache profile:

```rust
struct ReasoningProfileKey {
    provider_id: ProviderId,
    endpoint_id: EndpointId,
    api_style: ApiStyle,
    model_id: ModelId,
}

struct ReasoningCapability {
    mode_control: ThinkingModeControl,
    effort: Option<EffortControl>,       // field, accepted, aliases, default
    budget: Option<BudgetControl>,       // field, min/max, effort exclusivity/mapping
    preservation: PreservationControl,  // reasoning-content/tool replay requirements
    availability: CapabilityConfidence, // verified, conflicting_docs, unverified
}
```

Compile this semantic record into the vendor request only after endpoint resolution. Preserve an explicit `Unset` state so the provider, rather than Nexa, owns documented defaults. Unknown, HTTP, non-standard-port, or user-edited endpoints receive no trusted reasoning profile merely because the model string matches.

### 6.3 Recommended capability architecture

Separate cache semantics, usage decoding, and routing affinity:

```rust
struct PromptCacheProfileKey {
    provider_id: ProviderId,
    endpoint_id: EndpointId,
    api_style: ApiStyle,
    model_id: ModelId,
}

enum PromptCacheMode {
    None,
    ImplicitExactPrefix,
    ExplicitBreakpoints,
    ProviderSession,
}

struct PromptCacheCapability {
    mode: PromptCacheMode,
    min_cacheable_tokens: Option<u32>,
    max_breakpoints: Option<u8>,
    ttl_seconds: Option<u32>,
    lookback_content_blocks: Option<u32>,
    marker_targets: CacheMarkerTargets,
    tool_definitions_are_prefix: bool,
    requires_stable_tool_serialization: bool,
    usage_decoder: CacheUsageDecoderId,
}

struct RoutingAffinityCapability {
    mode: RoutingAffinityMode,
    max_session_id_bytes: Option<u16>,
    route_observer: RouteObserverId,
}
```

The shared model catalog may retain `promptCache: boolean` temporarily for compatibility, but adapters should consume a resolved profile, not re-implement provider/model string checks. Unknown/custom OpenAI-compatible endpoints resolve to `None/Unknown` unless the user or catalog explicitly declares a profile. This also prevents sending vendor-only marker fields to an unrelated endpoint that happens to serve a similarly named model.

Prompt IR should describe semantic stability rather than vendor wire fields:

```rust
enum PromptStability { Stable, Replayable, Volatile }
enum CacheBoundaryHint { PolicyEnd, StableEvidenceEnd, ReplayableTurnTail, LatestToolRound }
```

Each provider compiler translates those hints into no marker, message-content breakpoints, a provider session, or another supported mechanism. Tool order and canonical JSON serialization belong before the provider compiler so all exact-prefix strategies benefit.

### 6.4 Observability contract

OpenTelemetry's current GenAI span model includes provider, server address, request/response model, cache-read input tokens, cache-creation input tokens, and response time-to-first-chunk. The conventions are still marked development, so Nexa can align names while keeping its storage schema versioned. [OpenTelemetry GenAI span model](https://github.com/open-telemetry/semantic-conventions-genai/blob/fe5608e249d64bc5961329a82f8915fe95ced51a/model/gen-ai/spans.yaml#L17-L45), [response fields](https://github.com/open-telemetry/semantic-conventions-genai/blob/fe5608e249d64bc5961329a82f8915fe95ced51a/model/gen-ai/spans.yaml#L203-L225)

Persist one non-content record per model invocation:

```text
logicalProviderId, endpointId, apiStyle
requestedModelId, responseModelId, actualUpstreamProvider
cacheProfileId, cacheProfileVersion, promptSchemaVersion
stablePrefixHash, toolSurfaceWireHash, eligibleReusableTokens
markerPositions, interRequestGapMs
providerCacheReadTokens, providerCacheWriteTokens, providerCacheMissTokens
normalizedCacheReadTokens, normalizedCacheWriteTokens, normalizedCacheMissTokens
rawUsageFragment, usageDecoderId, usageCoverage
requestLatencyMs, timeToFirstChunkMs, timeToFirstVisibleOutputMs
routeStrategy, routeAttempt, routeGenerationId
cacheOutcomeReason
```

`rawUsageFragment` means the small provider usage/route object after secret and content scrubbing, not the prompt or completion. Keep raw and normalized values together so parser regressions can be diagnosed without mislabeling a provider-reported zero.

Suggested outcome vocabulary:

- `hit_reported`
- `cold_create_reported`
- `miss_reported`
- `ineligible_below_minimum`
- `unsupported_profile`
- `explicit_disabled_by_profile`
- `ttl_likely_expired`
- `prefix_changed`
- `tool_surface_changed`
- `route_changed`
- `usage_not_reported`
- `usage_schema_unknown`

Dashboards should show token-weighted hit rate and eligible-request hit rate separately, plus hit/miss time-to-first-chunk. Never compare DeepSeek's global best-effort exact-prefix hit rate directly with Qwen's five-minute explicit-cache rate without filtering to requests eligible under each profile.

### 6.5 What not to copy

- Do not put `cache_control` on Qwen tool definitions; Alibaba explicitly says those markers are ignored.
- Do not enable explicit Qwen caching by provider enum or model substring alone. Model, endpoint, region/API style, and current support table matter.
- Do not add a universal delay after DeepSeek calls. Cache construction is asynchronous/best-effort and the docs provide no constant that makes a global sleep correct.
- Do not assume an OpenRouter model slug identifies one upstream provider endpoint, and do not treat missing `openrouter_metadata` on a cache replay as a parser failure.
- Do not send `enable_thinking` and `thinking_budget` to every model behind Alibaba's OpenAI-compatible endpoint. K3, Qwen3.8, GLM, DeepSeek, and MiniMax do not share one wire contract.
- Do not equate `max` with an unbounded numeric token budget or treat `auto` as a reasoning level. Both terms have model- and parameter-specific meanings in the official contracts.
- Do not collapse standard pay-as-you-go and Token Plan merely because both currently list formal `qwen3.8-max`; their endpoints, keys, entitlements, and broader model allow-lists remain different.
- Do not retain Moonshot's discontinued `kimi-latest` and old K2 aliases as normal selectable models after their official retirement dates.
- Do not store normalized usage under a field named `provider_raw`; retain the actual scrubbed provider fragment and the normalized interpretation separately.
- Do not set a hard “Qwen must hit 90%” product gate before eligibility, TTL, route, and usage-parser coverage exist. First make every zero explainable; then set per-profile targets from production baselines.

## 7. Recommended delivery slices and proof

The five areas can share one product upgrade while remaining behavior-separated in commits:

| Slice | Core change | Required proof |
| --- | --- | --- |
| Turn timing | persisted lifecycle timestamps, local live badge, completed footer/Task Center projection | restart/cancel/timeout tests; React Profiler proves the transcript does not rerender each tick |
| File visuals | visual catalog v2, brand accent, audited SVG allow-list | light/dark/high-contrast snapshots; fallback and asset-license inventory tests |
| Motion | dedicated Slash surface, compact/heavy disclosure primitives | keyboard/ARIA tests; WebView trace with 64 results and large/live diffs; reduced-motion snapshots |
| Task Center | summary page/cursor, index-compatible SQL, lazy detail panels | 10,000-run fixture; `EXPLAIN QUERY PLAN`; request-count test; stale-selection cancellation test |
| Provider profiles | resolved cache/reasoning compilers, Qwen/Kimi fixes, OpenRouter affinity, raw+normalized telemetry | provider/endpoint/API-style/model matrix tests; golden request bodies for every reasoning family; effort/budget exclusivity tests; usage fixtures; route-drift/replay cases |

The proposed numeric budgets in `D:\Nexa.txt` are useful release targets but are not facts established by upstream projects. Capture the current baseline first, then freeze representative fixtures and compare the new implementation on the same machine/WebView/database. This prevents an attractive architecture change from being called a performance improvement without measured evidence.

### 7.1 Final protocol corrections from PR review

- Anthropic counts explicit tool, system, and message `cache_control` markers against one request-wide limit of four. The request compiler therefore reserves the tool marker first and caps the remaining stable-prefix markers across system and messages. [Anthropic prompt caching](https://platform.claude.com/docs/en/build-with-claude/prompt-caching)
- xAI's direct Chat Completions endpoint accepts top-level `reasoning_effort` for supported chat models. Grok 4.5 supports `low`, `medium`, and `high` with `high` as the default and cannot disable reasoning; Grok 4.3 supports `none`, `low`, `medium`, and `high`. The Grok 4.20 multi-agent model is Responses-only, so it must not be exposed through Nexa's Chat Completions catalog until a Responses adapter exists. [xAI reasoning](https://docs.x.ai/developers/model-capabilities/text/reasoning), [xAI multi-agent](https://docs.x.ai/developers/model-capabilities/text/multi-agent), [xAI May 2026 migration](https://docs.x.ai/developers/migration/may-15-retirement)
- MiniMax's direct OpenAI-compatible endpoint documents its M-series as native reasoning models and requires the complete assistant response, including thinking, to be replayed for multi-turn tool use. Nexa preserves that history in the provider's native `<think>` content form and does not invent an effort or token-budget control. [MiniMax OpenAI-compatible API](https://platform.minimax.io/docs/api-reference/text-openai-api)
- Mistral Medium 3.5 and Mistral Small support top-level `reasoning_effort`; the API enum is `none`, `minimal`, `low`, `medium`, `high`, or `xhigh`. Their responses can contain typed `thinking` and `text` chunks, which must be partitioned and replayed without flattening the thinking into visible output. Magistral 2509 is a deprecated native-reasoning model and does not accept this adjustable control. [Mistral reasoning](https://docs.mistral.ai/studio-api/conversations/reasoning), [Mistral Chat API](https://docs.mistral.ai/api), [Mistral native reasoning](https://docs.mistral.ai/resources/deprecated/native-reasoning)

## Source boundary

Only first-party specifications, official provider documentation, official design-system guidance, and source from the owning open-source repositories were used for protocol/library claims. Search-result summaries, blog aggregations, and third-party provider comparisons were not used as evidence.
