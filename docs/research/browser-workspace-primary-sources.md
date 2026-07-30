# Browser Workspace: Primary-source feasibility notes

Date: 2026-07-30

This note verifies the Browser Workspace proposal against the versions actually pinned by Nexa and against first-party implementation material from Tauri/Wry, Playwright/Chromium, and Electron. It is an engineering input for TODO phases 0–3, not a claim that phases 4–5 are already supported.

## Executive decision

The proposed visible-browser direction is valid, with three important qualifications:

1. A Tauri child webview is a real native webview whose remote URL is the top-level document, so it solves the `iframe`/`frame-ancestors` class of display failure. However, in Tauri 2.11.5 both `WebviewBuilder` and `Window::add_child` are gated by Tauri's `unstable` Cargo feature. Nexa pins Tauri exactly but does **not** currently enable that feature. [Nexa's pinned dependency](https://github.com/MLGBJDLW/Nexa/blob/9d7e409460902ad09151510bd5bc031def873887/apps/desktop/src-tauri/Cargo.toml#L9-L13), [Tauri 2.11.5 child-builder gate](https://github.com/tauri-apps/tauri/blob/tauri-v2.11.5/crates/tauri/src/webview/mod.rs#L257-L282), [Tauri 2.11.5 `add_child` gate](https://github.com/tauri-apps/tauri/blob/tauri-v2.11.5/crates/tauri/src/window/mod.rs#L1122-L1147)
2. Tauri 2.11.5 supplies enough generic APIs for a visible Phase 1 browser: initial URL, native bounds, show/hide/focus, navigate, reload, page-load/navigation/new-window/download callbacks, initialization scripts, and profile-related settings. It does **not** supply generic back/forward state, stop-loading, page screenshot, CDP session, input dispatch, or a permission-request broker. Those gaps materially narrow what can be promised in Phase 2. [Exact `WebviewBuilder` API](https://docs.rs/tauri/2.11.5/tauri/webview/struct.WebviewBuilder.html), [exact `Webview` API](https://docs.rs/tauri/2.11.5/tauri/webview/struct.Webview.html)
3. Tauri capabilities do deny IPC from remote pages unless a matching remote capability exists. The match is not “webview label only”: runtime authorization requires a matching local/remote execution context and then accepts either a matching webview label **or** a matching parent-window label. A capability that names the `main` window therefore covers every webview in that window for the origins configured on that capability. Browser capabilities must name `webviews` and omit `windows`. [Tauri capability reference](https://v2.tauri.app/reference/acl/capability/), [runtime matching implementation](https://github.com/tauri-apps/tauri/blob/tauri-v2.11.5/crates/tauri/src/ipc/authority.rs#L439-L468)

The practical recommendation is: complete Phase 0; accept the pinned `unstable` dependency explicitly for Phase 1; implement Phase 2 initially as a visible-webview `DomEval` backend with honest feature flags; and make unsupported actions require user takeover. Do not silently call the existing headless Chromium backend while describing it as the same visible tab.

## 1. Version baseline in this repository

At commit `9d7e409460902ad09151510bd5bc031def873887`, the desktop crate fixes `tauri = 2.11.5` and `tauri-build = 2.6.3`. Its enabled Tauri features are only `tray-icon` and `protocol-asset`; `unstable` is absent. [Pinned `Cargo.toml`](https://github.com/MLGBJDLW/Nexa/blob/9d7e409460902ad09151510bd5bc031def873887/apps/desktop/src-tauri/Cargo.toml#L9-L13)

The current lockfile resolves the relevant implementation stack to `tauri-runtime-wry = 2.11.4`, `wry = 0.55.1`, and `tao = 0.35.3`. The fixed Wry source is used below where Tauri delegates platform behavior. [Wry 0.55.1 source tag](https://github.com/tauri-apps/wry/tree/wry-v0.55.1)

`tauri-build` is build-time ACL/schema generation, not an alternate browser runtime. Nexa currently calls `tauri_build::build()` without an `AppManifest`, so custom application commands retain Tauri's default local-command behavior. Tauri's capability documentation recommends enumerating custom commands with `AppManifest::commands` when they need ACL treatment. [Tauri capabilities, “Core Permissions”](https://v2.tauri.app/security/capabilities/#core-permissions), [`tauri-build` 2.6.3 `AppManifest` source](https://github.com/tauri-apps/tauri/blob/tauri-v2.11.5/crates/tauri-build/src/acl.rs#L88-L109)

## 2. Tauri 2.11.5 capability audit

### 2.1 Child webview creation and bounds

`WebviewBuilder::new(label, WebviewUrl::External(url))` plus `window.add_child(builder, position, size)` is the intended child-webview construction path. `add_child` sets the initial native rectangle and marshals creation through the event-loop main thread. The public builder and the public `add_child` method exist only when the `unstable` feature is enabled on desktop. [Builder and examples](https://github.com/tauri-apps/tauri/blob/tauri-v2.11.5/crates/tauri/src/webview/mod.rs#L284-L360), [`add_child` implementation](https://github.com/tauri-apps/tauri/blob/tauri-v2.11.5/crates/tauri/src/window/mod.rs#L1122-L1147)

After construction, `Webview` exposes `set_bounds`, `set_size`, `set_position`, `bounds`, `position`, `size`, `hide`, `show`, and `set_focus`. These APIs are generic desktop APIs even though child creation is unstable. [Tauri 2.11.5 bounds and visibility methods](https://github.com/tauri-apps/tauri/blob/tauri-v2.11.5/crates/tauri/src/webview/mod.rs#L1497-L1590)

Implementation implication: React should own the BrowserDock layout, but it must report the content rectangle to Rust; Rust owns the native child-webview rectangle. Update it after split-pane resize, window resize, maximize/fullscreen changes, and DPI/scale changes. Hiding a dock should call `hide`, not destroy the tab, so the document and profile state remain alive. This is an architectural inference from the native bounds/visibility API above.

`WebviewWindowBuilder` is stable and can host a single webview in a separate native window. It is a useful fallback or pop-out mode, but it is not the requested in-window BrowserDock. [Tauri 2.11.5 `WebviewWindowBuilder`](https://docs.rs/tauri/2.11.5/tauri/webview/struct.WebviewWindowBuilder.html)

### 2.2 Navigation, reload, back, and forward

`Webview::navigate(Url)` and `Webview::reload()` are public generic methods. The builder also provides `on_navigation`, which receives the destination and cancels the navigation when it returns `false`, plus `on_page_load` for started/finished load events. [Navigation/reload source](https://github.com/tauri-apps/tauri/blob/tauri-v2.11.5/crates/tauri/src/webview/mod.rs#L1680-L1698), [navigation and page-load hooks](https://github.com/tauri-apps/tauri/blob/tauri-v2.11.5/crates/tauri/src/webview/mod.rs#L499-L676)

There is no generic `go_back`, `go_forward`, `can_go_back`, `can_go_forward`, or `stop` method in Tauri 2.11.5's `Webview` API. The options are:

- use `Webview::eval("history.back()")` / `history.forward()` as a cross-engine minimum, accepting that it provides no reliable native “can go” state;
- use `Webview::with_webview` and implement platform adapters for WebView2, WebKitGTK, and WKWebView; Tauri explicitly warns that these raw platform crates may change in Tauri minor releases; or
- defer authoritative enabled/disabled history state until the platform adapters exist.

The absence is visible in the complete versioned method list, and the raw-platform escape hatch is documented in source. [Exact Tauri 2.11.5 `Webview` methods](https://docs.rs/tauri/2.11.5/tauri/webview/struct.Webview.html), [`with_webview` warning and platform handles](https://github.com/tauri-apps/tauri/blob/tauri-v2.11.5/crates/tauri/src/webview/mod.rs#L1600-L1680)

Do not implement a second application-maintained history stack and call it browser history: SPA `pushState`, redirects, fragment navigation, form resubmission, and popup/opener behavior will diverge from the engine's actual history.

### 2.3 Initialization scripts and the IPC boundary

`initialization_script` runs after the global object exists but before document parsing and page scripts, on every top-level navigation. `initialization_script_for_all_frames` additionally targets child frames. Tauri warns that the script should guard `window.location`; it also documents that on Windows a “main-frame-only” initialization script is added to subframes anyway. [Tauri 2.11.5 initialization-script contract](https://github.com/tauri-apps/tauri/blob/tauri-v2.11.5/crates/tauri/src/webview/mod.rs#L816-L940)

These scripts run in page JavaScript, not in a Playwright-style private control plane. A remote page can inspect or tamper with globals installed in its world. `event.composedPath()` helps identify nodes inside an open shadow tree, but it does not make the picker trusted or reveal closed shadow-root internals. Playwright independently documents the same closed-shadow limitation. [Playwright Shadow DOM locator behavior](https://playwright.dev/docs/locators#locate-in-shadow-dom)

Tauri injects `window.__TAURI_INTERNALS__`, its invoke system, and current window/webview metadata into the main frame of every managed webview. Incoming invokes are then checked against the invoke key and runtime ACL; remote invokes without an explicit matching remote capability are rejected. [Tauri injection source](https://github.com/tauri-apps/tauri/blob/tauri-v2.11.5/crates/tauri/src/manager/webview.rs#L157-L198), [remote invoke authorization](https://github.com/tauri-apps/tauri/blob/tauri-v2.11.5/crates/tauri/src/webview/mod.rs#L1742-L1845)

`WebviewBuilder` does not expose a separate custom IPC handler. Wry 0.55.1 does expose `WebViewBuilder::with_ipc_handler`, but Tauri's Wry runtime consumes that hook for Tauri invoke processing. Replacing/bypassing it means owning a lower-level Wry host rather than using Tauri's child-webview runtime. [Wry custom IPC API](https://github.com/tauri-apps/wry/blob/wry-v0.55.1/src/lib.rs#L1127-L1144), [Tauri runtime wiring](https://github.com/tauri-apps/tauri/blob/tauri-v2.11.5/crates/tauri-runtime-wry/src/lib.rs#L5158-L5177)

For Phase 3 there are therefore two defensible bridge choices:

- **Zero remote Tauri permission:** the injected picker stores an observation-scoped artifact in page memory and the host retrieves it with `eval_with_callback`. This is pull-based and must still treat the page result as untrusted, but the page receives no native command.
- **One narrow remote ingress command:** enumerate one application command in the build `AppManifest`, grant only that command to `webviews: ["browser-*"]` for deliberately selected remote URL patterns, return no privileged data, derive session/tab/current URL from the invoking `Webview` on the Rust side, validate and size-limit the payload, and rate-limit it. The page can forge picker messages, so this must be an untrusted event intake, never an authority decision.

The first choice is the safer Phase 3 baseline. A general-purpose `invoke`, event, filesystem, shell, dialog, updater, or process bridge must never be exposed to remote pages.

### 2.4 Navigation, new-window, download, and permission hooks

The child builder has these relevant hooks:

| Hook | Verified behavior | Consequence |
| --- | --- | --- |
| `on_navigation` | Destination URL; return `false` to cancel. | Enforce allowed schemes, external-protocol handoff, localhost policy, and audit events before navigation. |
| `on_new_window` | Handles `window.open`/new-window requests and returns `Allow`, `Deny`, or `Create { window: WebviewWindow }`. | It cannot directly return a new child webview/tab. “Deny then open a new BrowserDock tab” loses true opener semantics and can break OAuth. |
| `on_download` | `Requested` may set an absolute destination or return false; `Finished` reports success. | Enough to deny or broker a basic download, but no generic progress stream. On macOS the finished path is always absent. |
| `on_page_load` | Started/finished events and URL. | Use to advance document epochs and update toolbar state; “finished” is not proof that an SPA is idle. |

Sources: [Tauri 2.11.5 hook implementations](https://github.com/tauri-apps/tauri/blob/tauri-v2.11.5/crates/tauri/src/webview/mod.rs#L499-L676), [`NewWindowResponse` platform constraints](https://github.com/tauri-apps/tauri/blob/tauri-v2.11.5/crates/tauri/src/webview/mod.rs#L229-L255), [`DownloadEvent`](https://github.com/tauri-apps/tauri/blob/tauri-v2.11.5/crates/tauri/src/webview/mod.rs#L67-L107)

`NewWindowResponse::Create` additionally requires the popup to share the caller's WebView2 environment on Windows, related WebKit view/process on Linux, or WKWebView configuration on macOS. That is why “popup becomes an ordinary new child tab” is not a safe Phase 1 promise. [Tauri new-window constraints](https://github.com/tauri-apps/tauri/blob/tauri-v2.11.5/crates/tauri/src/webview/mod.rs#L229-L255)

Tauri 2.11.5 and Wry 0.55.1 do **not** expose a portable permission-request callback in their public builders. The fixed Wry implementation only adds a WebView2 `PermissionRequested` handler for clipboard-read when clipboard access is enabled. More seriously, Wry's WKUIDelegate returns `Grant` to WebKit media-capture permission requests; operating-system entitlements/prompts remain a separate layer, but Nexa cannot insert a per-origin application approval at that Wry hook through Tauri's generic builder. [Wry WebView2 clipboard handling](https://github.com/tauri-apps/wry/blob/wry-v0.55.1/src/webview2/mod.rs#L497-L515), [Wry WKWebView media decision](https://github.com/tauri-apps/wry/blob/wry-v0.55.1/src/wkwebview/class/wry_web_view_ui_delegate.rs#L122-L141)

Immediate safety rule: Phase 1 must not request camera/microphone/location entitlements or claim that site permissions are brokered. Permission-sensitive scenarios remain user takeover/external-browser cases until each platform has an audited adapter and tests. This is a concrete reason to leave permissions in TODO Phase 4.

### 2.5 Data directories and profiles

`WebviewBuilder::data_directory(PathBuf)`, `incognito(bool)`, and `data_store_identifier([u8; 16])` are present. The fixed Tauri runtime keys and reuses Wry `WebContext` objects by data-directory path, so multiple tabs assigned the same directory share the same engine context; distinct paths create distinct contexts. [Tauri builder settings](https://github.com/tauri-apps/tauri/blob/tauri-v2.11.5/crates/tauri/src/webview/mod.rs#L953-L1007), [Tauri runtime context map](https://github.com/tauri-apps/tauri/blob/tauri-v2.11.5/crates/tauri-runtime-wry/src/lib.rs#L4784-L4822), [Wry `WebContext` model](https://github.com/tauri-apps/wry/blob/wry-v0.55.1/src/web_context.rs#L12-L70)

Platform differences matter:

- Windows and Linux use the Wry context/data directory for persistent browser data.
- WKWebView does not support arbitrary data directories; Tauri/Wry provide `data_store_identifier` on macOS 14+/iOS 17+ instead.
- `incognito` uses WebView2 in-private mode when the installed runtime supports it, an ephemeral WebKit context on Linux, and a non-persistent WKWebsiteDataStore on Apple. It is unsupported on Android. [Tauri 2.11.5 profile-setting docs](https://docs.rs/tauri/2.11.5/tauri/webview/struct.WebviewBuilder.html#method.incognito), [Wry macOS data-store extension](https://github.com/tauri-apps/wry/blob/wry-v0.55.1/src/lib.rs#L1531-L1563)

Recommended Phase 1 mapping:

```text
BrowserProfile id -> platform profile key
  Windows/Linux: absolute app-owned data_directory
  macOS 14+: stable 16-byte data_store_identifier

BrowserSession id -> conversation ownership + profile id
BrowserTab id -> unique child webview label using the session profile
```

For a multi-tab temporary conversation profile, prefer a dedicated app-owned temporary profile directory that is shared by its tabs and deleted only after all webviews using it are closed. Do not promise that separately created incognito webviews share one temporary session until that behavior is verified on each engine. Never point Tauri at the user's normal Chrome/Edge profile.

### 2.6 Threading and unstable constraints

Tauri documents a Windows deadlock when webview/window creation is performed in synchronous commands or event handlers and recommends async commands or separate threads. `Window::add_child` itself dispatches the actual construction to the main thread and waits for the result. `with_webview` also runs its callback on the main thread. [Tauri builder known issue](https://github.com/tauri-apps/tauri/blob/tauri-v2.11.5/crates/tauri/src/webview/mod.rs#L284-L347), [`add_child` dispatch](https://github.com/tauri-apps/tauri/blob/tauri-v2.11.5/crates/tauri/src/window/mod.rs#L1129-L1147), [`with_webview` thread contract](https://github.com/tauri-apps/tauri/blob/tauri-v2.11.5/crates/tauri/src/webview/mod.rs#L1600-L1680)

Host commands that create/close/reparent webviews should therefore be async entry points, avoid holding `BrowserRuntime` mutexes while waiting on the main thread, and send immutable creation parameters into the UI-thread closure. The feature flag is explicitly named “unstable” by Tauri and may break in a future minor, so keeping the exact `=2.11.5` pin and adding compile/smoke coverage is required. [Tauri feature description](https://github.com/tauri-apps/tauri/blob/tauri-v2.11.5/crates/tauri/src/lib.rs#L10-L19)

## 3. Capabilities for remote child webviews

### 3.1 What matching actually does

A capability may target window-label globs, webview-label globs, local app content, and remote URL patterns. The official schema states that a matching window enables the capability for all webviews in that window regardless of `webviews`, while a matching webview enables it regardless of `windows`. Webviews present in multiple capabilities receive the union of their permissions. [Official capability reference](https://v2.tauri.app/reference/acl/capability/), [security-boundary warning](https://v2.tauri.app/security/capabilities/#security-boundaries)

The 2.11.5 runtime implements this as:

```text
origin matches Local or Remote(URLPattern)
AND
(webview-label glob matches OR parent-window-label glob matches)
```

Remote context matching and deny-by-default behavior are covered by Tauri's own unit tests. [Runtime authority implementation](https://github.com/tauri-apps/tauri/blob/tauri-v2.11.5/crates/tauri/src/ipc/authority.rs#L43-L66), [label/origin resolution](https://github.com/tauri-apps/tauri/blob/tauri-v2.11.5/crates/tauri/src/ipc/authority.rs#L439-L468), [remote-domain tests](https://github.com/tauri-apps/tauri/blob/tauri-v2.11.5/crates/tauri/src/ipc/authority.rs#L897-L1004)

Nexa's current `default` capability targets `windows: ["main"]` and contains powerful shell/dialog/updater/process permissions, but it has no `remote` section. It therefore applies to local app content and does not authorize a remote page. Preserve that property. [Nexa default capability](https://github.com/MLGBJDLW/Nexa/blob/9d7e409460902ad09151510bd5bc031def873887/apps/desktop/src-tauri/capabilities/default.json)

### 3.2 Required isolation rules

1. Do not add a `remote` block to Nexa's current `default` capability.
2. Label browser views independently, for example `browser-{session_id}-{tab_id}`.
3. If a later browser bridge needs a remote capability, use only `webviews: ["browser-*"]`; omit `windows` entirely.
4. Prefer no remote capability in Phase 1. If Phase 3 needs an ingress command, generate a dedicated application permission for exactly that command and enumerate custom commands through `tauri-build::AppManifest`, so “all local custom commands” does not remain an accidental fallback. [Tauri remote API access](https://v2.tauri.app/security/capabilities/#remote-api-access), [`AppManifest::commands`](https://github.com/tauri-apps/tauri/blob/tauri-v2.11.5/crates/tauri-build/src/acl.rs#L88-L109)
5. Treat the current URL reported by the invoking `Webview` as authoritative. Never authorize from a URL, session ID, tab ID, or origin supplied inside the page payload.
6. Add a test page that attempts every registered Tauri command from a remote browser view and assert denial, except for any intentionally introduced one-way picker ingress.

Capabilities constrain Tauri command exposure; they do not make remote page JavaScript trusted, isolate an initialization script from that page, prevent web-engine vulnerabilities, or validate application command logic. Tauri explicitly lists lax scopes, incorrect command checks, Rust bypasses, webview zero-days, and supply-chain compromise outside the protection boundary. [Tauri capability security boundaries](https://v2.tauri.app/security/capabilities/#security-boundaries)

## 4. Session, tab, observe–act–verify, frames, and stale targets

### 4.1 Adopt the Playwright entity model, not necessarily Playwright itself

Playwright's first-party model cleanly separates a `BrowserContext` (isolated session/profile), multiple `Page` objects (tabs or popups), and per-page `Frame` trees. A context can enumerate all pages and emits a `page` event for popups. [Playwright pages](https://playwright.dev/docs/pages), [Playwright `BrowserContext`](https://playwright.dev/docs/api/class-browsercontext)

Use the same stable identity split in `BrowserRuntime`:

```text
profileId   persistent or temporary cookie/storage boundary
sessionId   Nexa conversation ownership + policy + control lease
tabId       stable Nexa identity
webviewLabel current native child used by the tab
documentEpoch incremented when the main frame commits navigation
observationId immutable evidence version scoped to tab + documentEpoch
```

Do not expose a native webview pointer, CDP target ID, frame object, or DOM node ID as Nexa's durable tab/element identity. Those are backend handles and can be destroyed/recreated.

Chromium's protocol makes the same distinction: a target has a `TargetID`; attaching yields a separate `SessionID`; target-created/destroyed/crashed events change target availability; frames have their own IDs/tree. [CDP Target domain](https://chromedevtools.github.io/devtools-protocol/tot/Target/), [CDP Page frame tree](https://chromedevtools.github.io/devtools-protocol/tot/Page/#method-getFrameTree)

Playwright can open a raw CDP session for a page or frame, but only on Chromium-based browsers. That is an architectural reference, not a portable implementation path for Tauri's WebKitGTK/WKWebView backends. [Playwright `newCDPSession`](https://playwright.dev/docs/api/class-browsercontext#browser-context-new-cdp-session), [Playwright `CDPSession`](https://playwright.dev/docs/api/class-cdpsession)

### 4.2 Observe–act–verify contract

Every agent action should be a transaction:

1. **Observe:** from the visible child webview, collect one immutable record containing current URL/title, `documentEpoch`, viewport, ready state, frame inventory where observable, semantic candidates, selected diagnostics, and a screenshot only when the backend truthfully supports one. Assign `observationId` after all fields are collected.
2. **Act:** validate session ownership and control lease; ensure tab/document/observation still match; re-resolve the target from a locator recipe; check uniqueness, visibility, stable bounds, enabled/editable state, and hit target; then perform one bounded action.
3. **Verify:** wait for an explicit postcondition (URL change, element state, text, navigation epoch, download request, or bounded “DOM settled” policy), then emit a fresh observation. Never reuse the pre-action element ref.

This follows Playwright's first-party behavior: locators resolve a fresh current DOM element before every action, actions auto-wait for uniqueness/visibility/stability/event-receivability/enabled state, and web-first assertions retry until their postcondition or timeout. [Playwright locators](https://playwright.dev/docs/locators), [actionability checks](https://playwright.dev/docs/actionability), [retrying assertions](https://playwright.dev/docs/test-assertions)

An element artifact should therefore store an observation-scoped fast ref plus a locator recipe, not a durable DOM handle:

```text
sessionId, tabId, observationId, documentEpoch
mainUrl, framePath/frameUrl
role, accessibleName, textSnippet
testId, id, name, href, tag
openShadowPath
bounds, screenshotCrop (when supported)
```

Playwright discourages `ElementHandle` because it points to one particular DOM element and is auto-disposed when its frame navigates; a locator instead stores retrieval logic and re-resolves. [Playwright `ElementHandle`](https://playwright.dev/docs/api/class-elementhandle)

### 4.3 Stale-target policy

Reject and require a new observation when any of these is true:

- the session/control owner changed;
- the tab was closed or its native webview was recreated;
- the main frame's `documentEpoch` changed;
- the referenced frame detached/navigated;
- the locator now matches zero or multiple candidates;
- role/name/critical attributes no longer agree with the artifact;
- stable bounds/hit testing fail; or
- the observation exceeded the configured age.

For a same-document mutation, re-resolve the locator rather than trusting an old node index. If exactly one semantically equivalent candidate exists, continue; otherwise fail closed. This retains Nexa's useful observation token while avoiding a permanent dependency on array position. The current implementation uses observation-scoped refs, URL/content hashes, and index/role/name/bounds checks; that is a good safety baseline but its top-level `document.querySelectorAll` does not cover frames or shadow trees. [Current Nexa observation validation](https://github.com/MLGBJDLW/Nexa/blob/9d7e409460902ad09151510bd5bc031def873887/crates/core/src/tools/browser_session_tool.rs#L316-L440)

### 4.4 Frames and Shadow DOM

Each page has a main frame and may have nested frame objects. Playwright exposes frame attach/navigate/detach lifecycle and uses `FrameLocator` to resolve inside an iframe. Nexa should persist a frame path/fingerprint with the artifact and invalidate it on frame lifecycle changes. [Playwright frames guide](https://playwright.dev/docs/frames), [Playwright frame lifecycle](https://playwright.dev/docs/api/class-frame)

For open Shadow DOM, recursively traverse `element.shadowRoot` while collecting candidates and use `event.composedPath()` during point-out. Prefer role/accessibility name and test IDs over long CSS/XPath paths. Closed shadow roots are not inspectable through ordinary DOM locators; XPath does not pierce shadow roots. [Playwright locator guidance and Shadow DOM limits](https://playwright.dev/docs/locators#locate-in-shadow-dom)

An all-frame initialization script can run picker logic in child documents, but a cross-origin child cannot directly traverse its parent DOM. Route its minimal selection record to the main-frame collector with a deliberately narrow message protocol, record frame URL/path where determinable, and treat platform differences as test requirements. If injection or identity is unavailable, fall back to region/coordinates and screenshot evidence; do not fabricate a DOM locator.

### 4.5 What Tauri can and cannot observe/act portably

Tauri's generic cross-platform surface gives Nexa `eval`, `eval_with_callback`, URL, page-load events, and DOM initialization scripts. That is enough for a first visible `DomEval` backend that reads semantic DOM state and performs limited DOM actions in the actual user-visible tab. [Tauri evaluation APIs](https://github.com/tauri-apps/tauri/blob/tauri-v2.11.5/crates/tauri/src/webview/mod.rs#L1908-L1941)

Tauri 2.11.5 has no generic page screenshot, accessibility-tree, network-event, console-event, CDP, trusted input-dispatch, or stop-loading API. Wry 0.55.1's generic `WebView` likewise exposes load/evaluate/reload/bounds/cookies but not screenshot or full automation. [Exact Tauri `Webview` API](https://docs.rs/tauri/2.11.5/tauri/webview/struct.Webview.html), [Wry 0.55.1 `WebView` source](https://github.com/tauri-apps/wry/blob/wry-v0.55.1/src/lib.rs#L1984-L2160)

Consequences for Phase 2:

- A DOM `element.click()` or input/value/event sequence is not equivalent to trusted browser input and may fail on sites that require user activation or `isTrusted` events.
- Same-origin DOM access does not automatically cover cross-origin frames; the all-frame injected collector is required.
- The existing headless Chromium screenshot/network/console observation cannot simply be redirected to the visible Tauri webview.
- Windows may gain a richer optional adapter through raw WebView2/CDP APIs, but Playwright/CDP semantics remain Chromium-only; the portable contract must feature-detect them.
- Unsupported actions should request user takeover or external-browser handoff, not operate a second hidden tab and claim session equality.

## 5. Electron `WebContentsView` comparison only

Electron's `WebContentsView` is a main-process native `View` that owns/adopts a `WebContents`, can be attached to a `BaseWindow`, assigned bounds, and navigated directly. `WebContents` then exposes a stable session, navigation history, frame hierarchy, input dispatch, navigation/window events, and a Chromium debugger/CDP connection. This is a materially tighter built-in fit for a browser-centric visible-tab automation host. [Electron `WebContentsView`](https://www.electronjs.org/docs/latest/api/web-contents-view), [Electron `webContents`](https://www.electronjs.org/docs/latest/api/web-contents/), [Electron session](https://www.electronjs.org/docs/latest/api/session)

Electron also has a clearer isolated preload world through `contextIsolation`; its security guide still warns that arbitrary untrusted content is a severe risk and requires no Node integration, sandboxing, permission handlers, navigation/window restrictions, sender validation, and no broad APIs exposed to the page. [Electron context isolation](https://www.electronjs.org/docs/latest/tutorial/context-isolation), [Electron security checklist](https://www.electronjs.org/docs/latest/tutorial/security)

This does not justify a Phase 0–3 rewrite. It does justify keeping `BrowserRuntime` free of Tauri types so a future Electron/managed-Chromium backend could implement the same session/tab/observation/control-lease protocol. Electron is the strategic comparison when cross-platform Chromium/CDP parity becomes a measured product requirement.

## 6. Immediate plan for TODO phases 0–3

### Phase 0: safe and immediately implementable

1. Rename the current feature “Reader Preview” and stop using `embeddable` as a browser-availability decision.
2. Define engine-neutral core types: profile/session/tab IDs, tab lifecycle, navigation state, capability flags, observation/element artifact, control lease, and browser events.
3. Move the process-global `OnceLock<Mutex<HashMap<...>>>` behind an injected `BrowserRuntime`; keep the current headless implementation as an explicitly named backend during migration. [Current global registry](https://github.com/MLGBJDLW/Nexa/blob/9d7e409460902ad09151510bd5bc031def873887/crates/core/src/tools/browser_session_tool.rs#L120-L160)
4. Add `BackendCapabilities` such as `dom`, `screenshot`, `networkEvents`, `consoleEvents`, `trustedInput`, `downloads`, `permissionBroker`, `crossOriginFrames`, and `cdp`. UI and tools must not infer these from platform names.
5. Decide and document acceptance of Tauri's `unstable` feature while retaining the exact version pin. Add a compile test that imports `WebviewBuilder`/`add_child` so a future dependency change fails loudly.
6. Before adding browser commands, enumerate application commands through `tauri_build::AppManifest` and add explicit local capability permissions. This prevents the browser bridge from being designed on top of an implicit command allowlist. [Tauri application-command ACL guidance](https://v2.tauri.app/security/capabilities/#core-permissions)

### Phase 1: implementable with the explicit unstable dependency

1. Enable `tauri/unstable` and build a `BrowserWebviewHost` around child creation, lookup, bounds, focus, show/hide, navigate, reload, close, and event hooks.
2. Use async Tauri commands; never hold the runtime registry mutex while waiting for child creation or a main-thread raw-webview callback.
3. Give every tab a unique `browser-*` webview label and every profile an app-owned data directory/platform data-store ID. All tabs in one persistent profile use the same key.
4. Keep the current main-window capability local-only. Add no remote capability for ordinary browsing.
5. Synchronize BrowserDock's measured rectangle into native child bounds. Explicitly test resize, maximize, fullscreen, scale-factor changes, hide/show, and focus return to Chat.
6. Implement URL entry, URL/title display, reload, and basic back/forward through an engine adapter. If the first implementation uses `history.back/forward`, do not claim reliable `canGoBack/canGoForward` until platform history adapters exist.
7. Handle new-window requests by policy: ordinary `_blank` links may be denied and opened as a new BrowserDock tab with an explicit “opener not preserved” limitation; OAuth/opener-dependent flows remain controlled `WebviewWindow` or external-browser/user-takeover cases.
8. Deny downloads by default in this phase or send them through a minimal user-confirmed broker. Do not claim progress or permission management.
9. Acceptance pages: Wikipedia/XFO, a React/Vue/Next SPA, cookies across reload and same-profile tabs, profile isolation, WebSocket, target-blank, IME, and a remote page attempting Tauri invokes.

### Phase 2: implement a truthful shared-session minimum

1. Make the visible Tauri child the authoritative tab. Bind the tool adapter to the same `tabId`/webview label; do not synchronize a hidden Chromium clone.
2. Start with a portable `DomEvalBackend`: URL/title/ready state/viewport/text/semantic elements plus limited click/type/select/press/scroll implemented against the visible document. Every result advertises that screenshot/network/console/trusted input may be unavailable.
3. Preserve existing observe/action/verify semantics, but replace durable node indices with observation-scoped refs plus fresh locator recipes.
4. Add a user/agent control lease. Any pointer/keyboard activity from the user revokes the agent lease and all outstanding observations. Every agent action reacquires or verifies the lease.
5. Add explicit postconditions and a fresh post-action observation. Do not use fixed sleeps as success evidence; this follows Playwright's actionability and web-first assertion model. [Playwright actionability](https://playwright.dev/docs/actionability)
6. Keep the existing headless backend available only under an explicit non-shared backend identity until it can be retired. If a requested operation exceeds visible-backend capabilities, report that and request takeover; do not switch invisibly.
7. Optionally prototype a Windows WebView2/CDP adapter behind capability flags. It must not change the cross-platform protocol or become an unconditional claim about macOS/Linux.

### Phase 3: implementable with bounded claims

1. Inject a small origin-guarded picker script. Use capture-phase pointer listeners, `composedPath`, a non-interactive overlay, Escape cancellation, open-shadow traversal, and clean teardown.
2. Prefer the zero-remote-permission pull bridge via `eval_with_callback`. If product latency later requires push, introduce exactly one audited remote ingress command on `webviews: ["browser-*"]` and treat its payload as attacker-controlled.
3. Emit the full `BrowserElementArtifact`: session/tab/observation/document epoch, URL/title, frame path, role/name/text, locator fingerprint, bounds, and optional crop.
4. Re-resolve before use and fail closed on ambiguity. Agent-to-user highlight uses the same locator recipe and current observation rather than a stale CSS path.
5. Support same-origin frames and open shadow roots first. Cross-origin frames are supported only where the all-frame injected bridge is proven on that platform.
6. Always provide region/coordinate fallback for canvas, WebGL, video, closed shadow roots, injection failure, and inaccessible frames. Without a portable Tauri screenshot API, region capture may initially require an OS-level window capture and must be labeled as such.
7. Security tests must have hostile page JavaScript forge picker messages, mutate globals, navigate frames during selection, spam the bridge, and attempt every Tauri command.

## 7. Limits that must remain explicit

The following cannot be safely promised for TODO phases 0–3 on the fixed Tauri/Wry stack:

- Full Chrome compatibility on every platform: Tauri uses the installed system webview rather than one bundled Chromium. [Tauri webview versions](https://v2.tauri.app/reference/webview-versions/)
- Portable CDP/Playwright attachment to the visible tab: CDP is Chromium-only, while Tauri also uses WebKitGTK and WKWebView. [Playwright CDP limitation](https://playwright.dev/docs/api/class-browsercontext#browser-context-new-cdp-session)
- Screenshot/network/console/accessibility parity with the current headless `browser_session` through generic Tauri APIs: the public `Webview` surface provides evaluation/navigation/window operations, not a portable browser-debugging protocol. [Tauri `Webview` method inventory](https://docs.rs/tauri/2.11.5/tauri/webview/struct.Webview.html)
- Trusted-input parity from JavaScript DOM actions: Playwright's actionability model and Chromium's input-dispatch protocol are capabilities above a plain in-page `eval` bridge. [Playwright actionability](https://playwright.dev/docs/actionability), [CDP Input domain](https://chromedevtools.github.io/devtools-protocol/tot/Input/)
- A portable site-permission approval broker in Tauri 2.11.5/Wry 0.55.1: Wry's Windows permission handler is an internal clipboard-read special case, not a public per-site broker. [Wry WebView2 permission handling](https://github.com/tauri-apps/wry/blob/wry-v0.55.1/src/webview2/mod.rs#L497-L515)
- Transparent conversion of every OAuth/new-window flow into a BrowserDock tab while preserving opener/environment semantics: Tauri's `Create` result accepts a `WebviewWindow`, and its platform-specific relationship constraints are explicit. [Tauri new-window API](https://github.com/tauri-apps/tauri/blob/tauri-v2.11.5/crates/tauri/src/webview/mod.rs#L229-L255), [Tauri new-window builder hook](https://github.com/tauri-apps/tauri/blob/tauri-v2.11.5/crates/tauri/src/webview/mod.rs#L634-L676)
- DOM locators for canvas/WebGL/video/remote desktops/closed shadow roots: locator piercing stops at closed shadow roots, and pixels without DOM semantics need a separate coordinate/vision path. [Playwright locator shadow-DOM limits](https://playwright.dev/docs/locators#locate-in-shadow-dom)
- Guaranteed automation of CAPTCHA, DRM, WebAuthn/passkeys, client certificates, extension-dependent sites, or OS-mediated file/credential pickers. These are deliberately outside the portable Phase 0–3 contract; even Playwright documents client certificates and extensions as explicit, engine/context-specific setup rather than universal page behavior. [Playwright client certificates](https://playwright.dev/docs/api/class-browser#browser-new-context-option-client-certificates), [Playwright Chrome extensions](https://playwright.dev/docs/chrome-extensions)
- Safe direct reuse of the user's normal Chrome/Edge profile: Wry accepts an application data directory/context configuration, but that does not establish safe concurrent ownership of another browser's profile. Use app-owned directories. [Wry `WebContext`](https://github.com/tauri-apps/wry/blob/wry-v0.55.1/src/web_context.rs#L12-L70), [Wry data-directory option](https://github.com/tauri-apps/wry/blob/wry-v0.55.1/src/lib.rs#L1531-L1563)
- “Remote webview has zero native authority” if a broad remote capability or general-purpose bridge is later added. Capability isolation is only as narrow as its URL, label, permission, scope, and Rust implementation. [Tauri capability reference](https://v2.tauri.app/reference/acl/capability/), [Tauri runtime capability matching](https://github.com/tauri-apps/tauri/blob/tauri-v2.11.5/crates/tauri/src/ipc/authority.rs#L439-L468)

These limits should appear in the runtime capability flags, product copy, and acceptance tests—not only in design documentation.

## 8. Go/no-go gates before merging Phase 1 or Phase 2

Phase 1 is a go only when:

- the exact Tauri pin plus `unstable` compiles on Windows, macOS, and Linux CI;
- remote pages cannot invoke any Tauri/application/plugin command;
- native bounds and focus remain correct across dock/window/DPI changes;
- profiles show positive sharing within a profile and negative isolation across profiles;
- popup, download, and permission behavior is deny-by-default and honestly surfaced; and
- closing/reopening the dock does not destroy the session unless explicitly requested.

Phase 2 is a go only when:

- UI and agent resolve the same `sessionId` and `tabId` to the same live child webview;
- user input invalidates the agent lease/observation;
- stale document/frame/element references fail closed;
- every agent action returns a fresh verified observation;
- backend capability omissions are visible to both tool routing and UI; and
- no fallback silently operates a separate headless page while claiming shared state.

That sequencing delivers the valuable BrowserDock early without overstating what Tauri 2.11.5 can automate portably.
