# Desktop lifecycle and Companion animation: primary-source architecture review

Date: 2026-08-11 (Asia/Shanghai)
Status: dated implementation research; not normative architecture
Scope: Nexa's Tauri tray/window lifecycle, Codex-compatible Companion packs, animation timing, desktop locomotion, reduced motion, and rendering cost.
Non-scope: this note does not change production code and does not claim that the closed-source Codex desktop renderer was inspected.

Statements marked **Fact** are directly supported by the linked first-party source or by the explicitly identified local diagnostic. Statements marked **Nexa inference** are implementation conclusions, not claims about upstream intent.

## Executive conclusion

The two reported symptoms have separate concrete causes but the same architectural theme: lifecycle state has more than one owner.

1. **Direct close is currently a window operation, not an application-exit operation.** At the beginning of this audit, `WindowCloseBehavior::Exit` resolved to closing only the `main` window. The tray and independent Companion window can therefore keep the Tauri process alive. Tauri's first-party contract provides the missing application boundary: `AppHandle::exit(0)` requests `RunEvent::ExitRequested` and then `RunEvent::Exit`, and Tauri's official tray example routes its Quit item through that method.
2. **The one-second pet flash is an atlas-normalization defect, not an FPS shortage.** Nexa's Codex fallback currently makes all eight cells in every row an equal-duration loop. A read-only visual audit of the locally installed `dorothy` and `yae-miko` v2 sheets confirmed that their idle row has visible art in columns 0-6 and a transparent column 7. At the current 8 FPS fallback, that empty cell is selected for about 125 ms once per one-second loop. Several action rows contain still more intentional empty tail cells.
3. **"Running" pose and physical locomotion are conflated.** Nexa maps thinking/tool activity to a running spritesheet row even when no movement controller is active. Meanwhile automatic walking is disabled by default, waits ten seconds, and is allowed only while the durable task state is idle. The result is often a character running in place and never translating while an agent is active.

The recommended repair is not a larger interval or more React state. It is two explicit owners:

- a native `DesktopLifecycle` authority that owns `RunningVisible -> RunningInTray -> Quitting -> Exited`, and through which the title-bar close, tray Quit, updater/restart, and OS exit all pass;
- a long-lived `CompanionEngine` that owns one decoded asset, one monotonic animation clock, a versioned animation-track table, and an independent locomotion machine. React projects controls and labels; it does not own each animation frame.

## Source identity, snapshots, and limits

| Source | Why it is in scope | Snapshot | License / availability |
| --- | --- | --- | --- |
| OpenAI Codex CLI | The official repository identifies Codex CLI as OpenAI's local coding agent ([README](https://github.com/openai/codex/blob/2cc9dbb9846b2dc03948414df6712adb967c70eb/README.md#L1-L10)); its current public TUI contains a first-party pet animation model | [`2cc9dbb9846b2dc03948414df6712adb967c70eb`](https://github.com/openai/codex/commit/2cc9dbb9846b2dc03948414df6712adb967c70eb) | [Apache-2.0](https://github.com/openai/codex/blob/2cc9dbb9846b2dc03948414df6712adb967c70eb/LICENSE#L1-L10) |
| Pi coding agent | The former `badlogic/pi-mono` slug resolves to canonical `earendil-works/pi`; its README calls it the Pi Agent Harness and home of its coding agent ([README](https://github.com/earendil-works/pi/blob/cd6852a123f2c0cc646a41a2a52f3711a603b822/README.md#L12-L20)) | [`cd6852a123f2c0cc646a41a2a52f3711a603b822`](https://github.com/earendil-works/pi/commit/cd6852a123f2c0cc646a41a2a52f3711a603b822) | [MIT](https://github.com/earendil-works/pi/blob/cd6852a123f2c0cc646a41a2a52f3711a603b822/LICENSE#L1-L10) |
| Nous Research Hermes Agent | The official repository identifies Hermes Agent and Hermes Desktop and says it is built by Nous Research ([README](https://github.com/NousResearch/hermes-agent/blob/9d6c5a920c773f86fad9ea16528212faeaa21815/README.md#L1-L14)) | [`9d6c5a920c773f86fad9ea16528212faeaa21815`](https://github.com/NousResearch/hermes-agent/commit/9d6c5a920c773f86fad9ea16528212faeaa21815) | [MIT](https://github.com/NousResearch/hermes-agent/blob/9d6c5a920c773f86fad9ea16528212faeaa21815/LICENSE#L1-L10) |
| Tauri | Nexa's native shell dependency and therefore the authority for tray and exit semantics | [`448d39ee25c4bcbf4bd40129abc5399213dcc0a9`](https://github.com/tauri-apps/tauri/commit/448d39ee25c4bcbf4bd40129abc5399213dcc0a9) | [MIT or Apache-2.0](https://github.com/tauri-apps/tauri/blob/448d39ee25c4bcbf4bd40129abc5399213dcc0a9/README.md#L1-L6) |
| OpenAI Skills / Hatch Pet | The public first-party Codex pet authoring contract, used only to establish the published compatibility boundary | [`49f948faa9258a0c61caceaf225e179651397431`](https://github.com/openai/skills/commit/49f948faa9258a0c61caceaf225e179651397431) | [Apache-2.0](https://github.com/openai/skills/blob/49f948faa9258a0c61caceaf225e179651397431/skills/.curated/hatch-pet/LICENSE.txt#L1-L10) |

**Important Codex boundary.** The public Codex code reviewed here is the CLI/TUI pet runtime. Its catalog explicitly says it is ported from the Codex App but currently fixes the public grid at 192x208 cells in an 8x9 atlas ([catalog](https://github.com/openai/codex/blob/2cc9dbb9846b2dc03948414df6712adb967c70eb/codex-rs/tui/src/pets/catalog.rs#L1-L8)). OpenAI's public Hatch Pet contract also still specifies 1536x1872, 8x9 and documents the nine public animation rows ([contract](https://github.com/openai/skills/blob/49f948faa9258a0c61caceaf225e179651397431/skills/.curated/hatch-pet/references/codex-pet-contract.md#L1-L35), [row timing](https://github.com/openai/skills/blob/49f948faa9258a0c61caceaf225e179651397431/skills/.curated/hatch-pet/references/animation-rows.md#L1-L29)). Neither source is evidence for every behavior of the closed desktop v2 8x11 renderer. Nexa should call its 8x11 support a compatibility dialect, keep it versioned, and avoid claiming undocumented upstream behavior.

## 1. Local evidence and failure mechanism

### 1.1 Direct close can leave a live tray process

**Fact, local audit at turn start.** `apps/desktop/src-tauri/src/main.rs` already distinguished `CloseWindow`, `MinimizeToTray`, and `ExitApplication`, but mapped the user's `WindowCloseBehavior::Exit` selection to `CloseWindow`. Its `CloseRequested` branch prevented and hid only the minimize-to-tray case. Closing the main window therefore did not request application exit; the separately created tray and Companion window could remain alive.

**Fact.** Tauri defines `RunEvent::ExitRequested` as the application-about-to-exit event and says a programmatic `AppHandle::exit` supplies an exit code; `RunEvent::Exit` means the event loop is exiting ([RunEvent contract](https://github.com/tauri-apps/tauri/blob/448d39ee25c4bcbf4bd40129abc5399213dcc0a9/crates/tauri/src/app.rs#L215-L240)). `AppHandle::exit` is documented in source as triggering both events ([exit implementation](https://github.com/tauri-apps/tauri/blob/448d39ee25c4bcbf4bd40129abc5399213dcc0a9/crates/tauri/src/app.rs#L566-L580)). The first-party tray example's Quit handler calls `app.exit(0)`, while Show/Hide remain window operations ([tray example](https://github.com/tauri-apps/tauri/blob/448d39ee25c4bcbf4bd40129abc5399213dcc0a9/examples/api/src-tauri/src/tray.rs#L48-L80)).

**Fact.** After the runtime emits its final exit event, Tauri calls unified application cleanup; that cleanup clears the tray-icon registry and, on Windows, hides remaining windows before dropping resources ([exit cleanup dispatch](https://github.com/tauri-apps/tauri/blob/448d39ee25c4bcbf4bd40129abc5399213dcc0a9/crates/tauri/src/app.rs#L1425-L1432), [cleanup implementation](https://github.com/tauri-apps/tauri/blob/448d39ee25c4bcbf4bd40129abc5399213dcc0a9/crates/tauri/src/app.rs#L1106-L1120)).

**Nexa inference.** A close preference named `exit` must call the application boundary. Destroying the main webview is not an equivalent substitute once tray, pet, updater, browser sessions, or background agents exist. Nexa should close its own services/helper windows in ordered exit hooks and let Tauri's final cleanup own destruction of the native tray object.

### 1.2 The one-second flash is reproducible from the normalized track

**Fact, local code.** Nexa's Codex fallback `default_codex_animations()` currently constructs every row as all eight columns. Idle is row 0 at 8 FPS, so the loop period is exactly one second. The renderer then advances through those normalized indices and writes `background-position` on a React state update.

**Fact, read-only local visual diagnostic.** The installed `dorothy` and `yae-miko` v2 WebP sheets are valid 8x11 atlases. In both, idle columns 0-6 contain a character and column 7 is transparent. Waving and jumping rows also have intentional transparent tails. No resource was modified and no diagnostic artifact was committed.

**Nexa inference.** For those packs, the present normalized idle track guarantees a transparent frame for one 8 FPS step every second. Increasing the cap to 24/30/60 cannot repair this; it only changes how often React checks the same wrong track. The first correctness gate is that a normalized track never includes padding cells.

### 1.3 Why there is little or no positional motion

**Fact, local code.** Nexa defaults `autoWalk` to false. When enabled, the current effect waits ten seconds, requires the projected task state and interactive behavior both to be idle, moves only horizontally for 2.8 seconds, and throttles native-window writes to one attempt every 50 ms. Thinking and tool states choose a running row but fail the locomotion precondition.

**Nexa inference.** Animation pose and locomotion need separate state. A `running-right` or `running-left` row should be selected because velocity is non-zero, not merely because an agent is thinking. Conversely, a working agent may roam if policy allows it without changing the durable agent state.

## 2. Upstream patterns worth adopting

### 2.1 Codex: explicit tracks, real frame counts, elapsed-time authority

**Fact.** Codex models an animation as explicit `AnimationFrame { sprite_index, duration }` values plus `loop_start` and a named fallback. Multiple frames do not automatically imply an infinite loop ([model](https://github.com/openai/codex/blob/2cc9dbb9846b2dc03948414df6712adb967c70eb/codex-rs/tui/src/pets/model.rs#L29-L59)). This is materially richer than Nexa's current `frames + one fps + looping` fallback.

**Fact.** Codex's default tracks do not assume eight meaningful cells per row. Idle has six explicitly timed frames with long holds. App-state tracks use per-state real frame counts, repeat the action a bounded number of times, and then enter the idle sequence ([default timing](https://github.com/openai/codex/blob/2cc9dbb9846b2dc03948414df6712adb967c70eb/codex-rs/tui/src/pets/model.rs#L584-L627)). The model also validates a 60 FPS ceiling and frame indices rather than accepting arbitrary geometry ([validation model](https://github.com/openai/codex/blob/2cc9dbb9846b2dc03948414df6712adb967c70eb/codex-rs/tui/src/pets/model.rs#L29-L42)).

**Fact.** The live `AmbientPet` is intended to persist: its source explicitly warns that repeatedly recreating the instance loses timing continuity and repeats cache work. A semantic notification mutates the instance and resets the clock only for the new animation ([persistent instance](https://github.com/openai/codex/blob/2cc9dbb9846b2dc03948414df6712adb967c70eb/codex-rs/tui/src/pets/ambient.rs#L138-L180)). Frame selection is computed from elapsed monotonic time, including loop-prefix arithmetic, and the runtime schedules only the delay until the next actual frame ([elapsed frame resolver](https://github.com/openai/codex/blob/2cc9dbb9846b2dc03948414df6712adb967c70eb/codex-rs/tui/src/pets/ambient.rs#L371-L412), [next-frame scheduling](https://github.com/openai/codex/blob/2cc9dbb9846b2dc03948414df6712adb967c70eb/codex-rs/tui/src/pets/ambient.rs#L196-L212)).

**Fact.** Codex has a regression test that reduced motion renders a stable first frame and schedules no follow-up ([test](https://github.com/openai/codex/blob/2cc9dbb9846b2dc03948414df6712adb967c70eb/codex-rs/tui/src/pets/ambient.rs#L505-L527)).

**Nexa inference.** Copy the timing model, not the terminal image protocol: explicit occupied frames, per-frame holds, a monotonic start epoch, exact fallback semantics, and no scheduled work in reduced-motion mode.

### 2.2 Pi: one coalesced render scheduler

**Fact.** Pi's TUI has one render scheduler with a 16 ms minimum interval and deduplicates repeated render requests through `renderRequested` and a single timer ([scheduler state](https://github.com/earendil-works/pi/blob/cd6852a123f2c0cc646a41a2a52f3711a603b822/packages/tui/src/tui.ts#L331-L346), [coalescing loop](https://github.com/earendil-works/pi/blob/cd6852a123f2c0cc646a41a2a52f3711a603b822/packages/tui/src/tui.ts#L757-L817)). Latency-sensitive keyboard input can preempt that throttled path with one immediate render, rather than creating a competing permanent loop ([immediate path](https://github.com/earendil-works/pi/blob/cd6852a123f2c0cc646a41a2a52f3711a603b822/packages/tui/src/tui.ts#L776-L790)). Its main-screen renderer writes synchronously but only repaints changed lines, with source comments tying that choice to avoiding spinner flicker ([differential output](https://github.com/earendil-works/pi/blob/cd6852a123f2c0cc646a41a2a52f3711a603b822/packages/tui/src/tui-main-screen.ts#L388-L424)).

**Nexa inference.** Projection events, hover, drag, sprite stepping, and settings refresh should invalidate one Companion render owner. They should not each restart a hook-local clock or create independent timers. Coalescing is useful even in a browser renderer because agent events can arrive much faster than a visible pet frame changes.

### 2.3 Hermes: stable canvas identity and independent roaming physics

Hermes is the closest first-party comparable because its current open repository contains both an agent desktop shell and a floating pet. It is not used here as a tray reference: the reviewed snapshot does not provide a completed tray lifecycle. It is useful for pet rendering and exit cleanup.

**Fact.** Hermes reads frequent pet-state changes through a ref/subscription inside one memoized canvas renderer. Its own source says this avoids React rerenders and keeps the component mounted until the pet asset itself changes ([stable renderer](https://github.com/NousResearch/hermes-agent/blob/9d6c5a920c773f86fad9ea16528212faeaa21815/apps/desktop/src/components/pet/pet-sprite.tsx#L109-L145), [memo boundary](https://github.com/NousResearch/hermes-agent/blob/9d6c5a920c773f86fad9ea16528212faeaa21815/apps/desktop/src/components/pet/pet-sprite.tsx#L368-L373)). It uses per-row real frame counts, touches the canvas only when the visible cell changes, uses a timeout for waits longer than 16 ms, and uses `requestAnimationFrame` only when a paint is due ([due-frame scheduler](https://github.com/NousResearch/hermes-agent/blob/9d6c5a920c773f86fad9ea16528212faeaa21815/apps/desktop/src/components/pet/pet-sprite.tsx#L209-L248), [draw loop](https://github.com/NousResearch/hermes-agent/blob/9d6c5a920c773f86fad9ea16528212faeaa21815/apps/desktop/src/components/pet/pet-sprite.tsx#L289-L356)).

**Fact.** Hermes computes those real row counts before the desktop renderer sees the sheet. Its source scans cells in order and stops a row at the first fully transparent tail frame, with an explicit comment that including blank tail cells causes a blank flash; it then publishes the counts as runtime metadata ([transparent-tail scan](https://github.com/NousResearch/hermes-agent/blob/9d6c5a920c773f86fad9ea16528212faeaa21815/agent/pet/render.py#L129-L176), [published counts](https://github.com/NousResearch/hermes-agent/blob/9d6c5a920c773f86fad9ea16528212faeaa21815/agent/pet/render.py#L202-L219)). Hermes' own constants remain an 8x9 dialect, so the algorithm is reusable but its row table is not a v2 authority ([Hermes grid boundary](https://github.com/NousResearch/hermes-agent/blob/9d6c5a920c773f86fad9ea16528212faeaa21815/agent/pet/constants.py#L1-L26)).

**Fact.** Hermes' roaming hook owns physics and direct DOM writes. It updates `left/top` every active frame without React rerenders, commits durable position only when the pet settles, and publishes motion pose/direction separately from other pet state ([ownership contract](https://github.com/NousResearch/hermes-agent/blob/9d6c5a920c773f86fad9ea16528212faeaa21815/apps/desktop/src/components/pet/use-pet-roam.ts#L60-L92)). It uses one RAF/timer scheduler, clamps `dt` after stalls, yields to drag, and separates pause/walk/jump/fall phases ([scheduler](https://github.com/NousResearch/hermes-agent/blob/9d6c5a920c773f86fad9ea16528212faeaa21815/apps/desktop/src/components/pet/use-pet-roam.ts#L162-L201), [physics step](https://github.com/NousResearch/hermes-agent/blob/9d6c5a920c773f86fad9ea16528212faeaa21815/apps/desktop/src/components/pet/use-pet-roam.ts#L270-L319)). Its test proves blur suspends movement and unmount clears all wake work ([lifecycle test](https://github.com/NousResearch/hermes-agent/blob/9d6c5a920c773f86fad9ea16528212faeaa21815/apps/desktop/src/components/pet/use-pet-roam.test.tsx#L139-L165)).

**Fact.** Hermes also treats its always-on-top overlay as an application-owned helper: the quit path explicitly closes the pet before backend/window teardown so it cannot float beyond application exit ([quit cleanup](https://github.com/NousResearch/hermes-agent/blob/9d6c5a920c773f86fad9ea16528212faeaa21815/apps/desktop/electron/main.ts#L12708-L12734)). It protects quit reentry with explicit prompt/confirmation latches before cleanup ([quit latches](https://github.com/NousResearch/hermes-agent/blob/9d6c5a920c773f86fad9ea16528212faeaa21815/apps/desktop/electron/main.ts#L2647-L2656), [before-quit gate](https://github.com/NousResearch/hermes-agent/blob/9d6c5a920c773f86fad9ea16528212faeaa21815/apps/desktop/electron/main.ts#L12633-L12684)).

**Nexa inference.** The valuable composition is: Tauri owns process exit; a persistent canvas owns frames; a locomotion controller owns position; semantic agent projection chooses non-locomotion poses. None of those owners should be replaced by React remounts.

### 2.4 Browser and accessibility standards

**Fact.** The HTML Standard defines animation-frame callbacks as an ordered callback map invoked with one high-resolution `now` timestamp ([WHATWG animation frames](https://html.spec.whatwg.org/multipage/imagebitmap-and-animations.html#animation-frames)). The Page Visibility specification says minimized or off-screen user agents can be `hidden` and defines `visibilitychange` as the lifecycle signal ([W3C Page Visibility](https://www.w3.org/TR/page-visibility-2/#visibility-states)). Media Queries Level 5 defines `prefers-reduced-motion: reduce` as a request to remove or replace motion that can cause discomfort or distraction ([W3C Media Queries](https://www.w3.org/TR/mediaqueries-5/#prefers-reduced-motion)).

**Nexa inference.** Use the RAF timestamp as the calculation input; do not advance by "one frame per callback." On hidden/minimized transition, cancel RAF and wake timers. On resume, reset the previous-step timestamp for physics and derive the sprite cell from the current track epoch. Reduced motion must disable both spritesheet stepping and physical translation, not merely a CSS keyframe.

## 3. Recommended Nexa architecture

### 3.1 Native `DesktopLifecycle` is the single exit authority

Use a small explicit state machine in `apps/desktop/src-tauri`, for example:

```text
RunningVisible --close/minimize--> RunningInTray
RunningVisible --close/exit-----> Quitting(reason=MainClose)
RunningInTray  --tray/show------> RunningVisible
RunningInTray  --tray/quit------> Quitting(reason=TrayQuit)
Running*       --update/restart-> Quitting(reason=Handoff)
Quitting       --Exit-----------> Exited
```

The implementation implications are:

1. `CloseRequested(main)` reads the saved close preference exactly once.
2. `minimize_to_tray` prevents the window close and hides the main window; it does not mark the application as quitting.
3. `exit` prevents an isolated main-window destroy, atomically marks `Quitting(MainClose)`, and calls the shared `request_app_exit` path.
4. Tray Quit calls that same shared path. It must not have separate cleanup semantics.
5. The quitting latch makes any later `CloseRequested` or repeated Quit idempotent and prevents close-to-tray logic from intercepting an actual application exit.
6. `ExitRequested` owns cancellable/ordered pre-exit work if needed; `Exit` owns final non-cancellable cleanup. Companion, browser sessions, watchers, and background runtimes are closed once.

This design also makes the settings label truthful: "Direct exit" means process exit, while "Keep in system tray" means window visibility only.

### 3.2 Normalize a versioned animation contract before rendering

The renderer should never infer `frames = every column` merely from grid width. Normalize every pack into tracks like:

```ts
type FrameStep = { spriteIndex: number; durationMs: number };
type AnimationTrack = {
  steps: FrameStep[];
  loopStart: number | null;
  fallback: string;
};
```

For a manifest that supplies explicit frames/timing, validate and preserve them. For Codex compatibility manifests that supply only `spriteVersionNumber`, use an audited, version-specific table of meaningful columns and timing. The 8x9 Codex public implementation can inform v1, but it must not silently stand in for v2. Add import-time alpha occupancy as a validation diagnostic: if a normalized step is fully transparent while another cell in the row is populated, reject or warn. Occupancy should catch contract/asset mismatch; it should not replace the versioned contract for intentionally subtle art.

Rows 9-10 in an 8x11 pack need a separate directional-look contract, not an accidental looping animation. If Nexa keeps advertising `directional_look_rows`, it should implement an explicit 16-direction lookup with cursor deadzone and angular hysteresis, or remove the advertised experimental capability until it does.

### 3.3 Keep one decoded surface and one monotonic clock

Replace per-frame React state with a persistent drawing surface:

- decode the WebP once per `(packId, contentHash)` and commit it atomically;
- keep the same canvas/element identity across projection polling and behavior changes;
- store effective semantic state, effective motion state, current track identity, and epoch in refs or a narrow runtime store;
- reset the epoch only when the effective track actually changes, not when an equivalent projection object arrives;
- select the visible step from elapsed time, so delayed callbacks skip ahead instead of slowing the animation;
- draw only when the selected sprite index changes;
- use a wake timer until the next step is near, then one RAF for paint alignment;
- keep the last committed surface visible through settings/asset refresh; swap only after decode succeeds.

This removes the two common meanings of "flash": a transparent padding cell and a transient fallback/remount during refresh.

### 3.4 Make locomotion an independent, latest-wins controller

The locomotion machine should own at least:

```text
phase: rest | walk | turn | jump | fall | dragged
position: x, y
velocity: vx, vy
facing: left | right
lastTickAt
target / bounds / monitorId
```

Agent state may request an expression (`waiting`, `review`, `failed`); locomotion selects directional run/jump rows only while motion requires them. Walking speed should be derived from sprite stride duration so feet and translation agree. Clamp `dt` after sleep/resume, re-read monitor work area on display changes, and persist only on settle/drag end rather than per tick.

Nexa must move a native transparent window to roam across the desktop, so blindly copying Hermes' in-window DOM writes is insufficient. Keep a single native position write in flight and a latest desired position. Coalesce to a measured 30 Hz (or the highest rate proven smooth on Windows), and when a write resolves immediately send the latest pending position. The current "drop every update while pending plus a fixed 50 ms gate" can produce uneven 20 Hz motion. Benchmark before choosing 60 Hz; native IPC and window-manager work are not free.

Product policy remains separate from engine correctness. If Smart interaction is meant to feel alive, expose locomotion as on by default for new users or make the disabled state obvious. Do not claim automatic movement while the shipped default is off.

## 4. Verification contract

The upgrade is complete only with behavior-level proof:

### Native lifecycle

- `Exit` close preference maps to application exit, never `CloseWindow`.
- title-bar direct exit and tray Quit invoke the same lifecycle owner.
- minimize-to-tray keeps the process, tray, active work, and optionally Companion alive according to `continueWhenMainHidden`.
- direct exit with the Companion visible produces `ExitRequested` then `Exit`, closes helper windows, and leaves no Nexa process or tray icon.
- repeated Quit/close during cleanup is idempotent; cleanup runs once.
- update/restart handoff cannot be converted into minimize-to-tray.

### Sprite correctness and continuity

- include a real-shape 8x11 fixture whose idle column 7 is transparent; assert that normalized idle never selects it;
- verify each versioned state uses only its meaningful frames and honors one-shot fallback;
- sample for at least several loop periods and assert no unexpected transparent frame;
- repeatedly deliver an equivalent projection and assert canvas/element identity, asset-read count, track epoch, and cadence remain stable;
- race a pack refresh and assert the old decoded pet stays visible until the replacement is decoded;
- simulate a late callback and assert elapsed-time selection skips to the correct cell;
- verify hidden/unfocused teardown leaves no RAF or wake timer, then resumes without a large physics jump;
- verify reduced motion renders one stable semantic frame, performs no physical translation, and schedules no animation work.

### Locomotion and performance

- position is monotonic toward a target, bounded to the active monitor work area, and turns exactly once at an edge;
- dragging synchronously suspends autonomous motion and resume starts from the dropped position;
- at most one native position command is in flight; the last desired coordinate is eventually applied;
- run-pose direction matches velocity and a stationary working pet never uses a locomotion row unless explicitly designed as an in-place action;
- idle rendering does not redraw at 60 Hz when no frame is due;
- collect Windows frame-time and native-position-write traces at 24/30/60 caps before selecting defaults.

## 5. Decisions to carry into implementation

1. Fix direct exit first; it is a correctness defect with a narrow authoritative Tauri path.
2. Fix normalized v2 tracks before tuning rendering. The local one-second flash has a deterministic transparent-cell explanation.
3. Move sprite stepping out of React render state into a persistent canvas/runtime owner.
4. Separate semantic agent state from locomotion pose and implement latest-wins native movement.
5. Treat 8x11 directional look as either a real, tested feature or an unsupported capability; do not leave it advertised but inert.
6. Keep reduced motion and hidden-window suspension as engine invariants, not CSS-only preferences.

No upstream is a complete blueprint. Tauri supplies process-exit authority, Codex supplies the strongest explicit animation-track semantics, Pi supplies a compact coalesced invalidation scheduler, and Hermes supplies the closest open pet renderer/roaming separation. The safe Nexa design is their deliberate composition with versioned Codex compatibility boundaries.
