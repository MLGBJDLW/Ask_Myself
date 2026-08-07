# Wave 3 AudioWorklet and Recording Dock primary-source research

Date: 2026-08-07

## Decision summary

Nexa should capture microphone PCM in an `AudioWorkletProcessor`, convert it
to fixed-duration PCM16 chunks on the audio rendering thread, and transfer each
chunk's `ArrayBuffer` to the renderer behind explicit credits. The renderer
must never own a complete recording. Native spooling remains the durable source
of truth; realtime transcription is a bounded secondary consumer that may
degrade without losing the accepted native prefix.

The chat composer should expose recording as one compact, responsive control
surface: duration and waveform, provider/language and transport state,
pause/resume, explicit stop-and-transcribe, and explicit cancel-and-delete.
At narrow widths the dock takes a full composer row while the message area,
not the composer, absorbs the height change.

This is an original Nexa implementation. No upstream implementation source was
copied.

## Source snapshots

All GitHub source links below are pinned to the 40-character commit observed on
2026-08-07. Moving branch links are intentionally not used as evidence.

| Source | Pinned revision | License / terms | Evidence used |
| --- | --- | --- | --- |
| [Web Audio API specification source](https://github.com/WebAudio/web-audio-api/blob/bfc7143fcf798a5a0fc056a2e31f78f679e219ea/index.bs) | `bfc7143fcf798a5a0fc056a2e31f78f679e219ea` | [W3C document terms](https://www.w3.org/copyright/document-license-2023/) | AudioWorklet rendering-thread execution, `process()` lifetime, and the bidirectional `MessagePort`. |
| [WHATWG HTML source](https://github.com/whatwg/html/blob/24c5e48bf66ea61bc199ec6338c81258275ba9c6/source) | `24c5e48bf66ea61bc199ec6338c81258275ba9c6` | [WHATWG copyright](https://whatwg.org/ipr-policy) | Structured serialization and transfer-list ownership semantics for transferable buffers. |
| [Media Capture and Streams source](https://github.com/w3c/mediacapture-main/blob/e0bde2206a9ac3af9536fe97208b4bd4c5b7aa1a/getusermedia.html) | `e0bde2206a9ac3af9536fe97208b4bd4c5b7aa1a` | [W3C document terms](https://www.w3.org/copyright/document-license-2023/) | Track `ended` lifecycle and the recommendation to stop costly capture resources. |
| [MediaStream Recording source](https://github.com/w3c/mediacapture-record/blob/40a620b091dd4c16a63bd6290fbcbbd14dbb0e9c/MediaRecorder.bs) | `40a620b091dd4c16a63bd6290fbcbbd14dbb0e9c` | [W3C document terms](https://www.w3.org/copyright/document-license-2023/) | `MediaRecorder`/Blob alternative considered and rejected for Nexa's PCM streaming boundary. |
| [CSS Flexible Box Layout source](https://github.com/w3c/csswg-drafts/blob/6af23215645075e9e88751765ea07d0d1231c5d6/css-flexbox-1/Overview.bs) | `6af23215645075e9e88751765ea07d0d1231c5d6` | [W3C document terms](https://www.w3.org/copyright/document-license-2023/) | Flex-item automatic minimum size and shrink behavior used for the narrow composer contract. |
| [WAI-ARIA source](https://github.com/w3c/aria/blob/846cd7d6ecb2fc445cd3186399d62c43c4ccdb5e/index.html) | `846cd7d6ecb2fc445cd3186399d62c43c4ccdb5e` | [W3C document terms](https://www.w3.org/copyright/document-license-2023/) | Named region and polite status semantics for changing recording state. |
| [GoogleChromeLabs MessagePort example](https://github.com/GoogleChromeLabs/web-audio-samples/blob/ecb78e13e9dd0ef6aa741794ff55c966be0c42fa/src/audio-worklet/basic/message-port/messenger-processor.js) | `ecb78e13e9dd0ef6aa741794ff55c966be0c42fa` | File header: BSD-style; repository [license](https://github.com/GoogleChromeLabs/web-audio-samples/blob/ecb78e13e9dd0ef6aa741794ff55c966be0c42fa/LICENSE): Apache-2.0 | Processor/node communication is intentionally asynchronous through the paired port. |
| [GoogleChromeLabs ring-buffer processor](https://github.com/GoogleChromeLabs/web-audio-samples/blob/ecb78e13e9dd0ef6aa741794ff55c966be0c42fa/src/audio-worklet/design-pattern/wasm-ring-buffer/ring-buffer-worklet-processor.js) | `ecb78e13e9dd0ef6aa741794ff55c966be0c42fa` | File header and repository: Apache-2.0 | Render quanta are adapted into an application-sized bounded buffer before downstream processing. |
| [LiveKit voice assistant control bar](https://github.com/livekit/components-js/blob/2da3e59e9854cde26cbeadcf8a5732ea42163bfa/packages/react/src/prefabs/VoiceAssistantControlBar.tsx) | `2da3e59e9854cde26cbeadcf8a5732ea42163bfa` | [Apache-2.0](https://github.com/livekit/components-js/blob/2da3e59e9854cde26cbeadcf8a5732ea42163bfa/LICENSE) | Microphone control, compact visualizer, and device menu are one coherent control group. |
| [LiveKit media-device selection hook](https://github.com/livekit/components-js/blob/2da3e59e9854cde26cbeadcf8a5732ea42163bfa/packages/react/src/hooks/useMediaDeviceSelect.ts) | `2da3e59e9854cde26cbeadcf8a5732ea42163bfa` | [Apache-2.0](https://github.com/livekit/components-js/blob/2da3e59e9854cde26cbeadcf8a5732ea42163bfa/LICENSE) | Device labels are refreshed only after permission and selection has an explicit default. |

## Evidence-to-design mapping

### 1. Audio rendering owns conversion; the renderer owns policy

The Web Audio specification puts `AudioWorkletProcessor` on the audio rendering
thread and gives the node and processor a paired `MessagePort`. The Chromium
sample follows exactly that seam: the processor performs time-sensitive work
and communicates asynchronously instead of touching UI state.

Nexa therefore:

- loads a separately bundled worklet with `audioWorklet.addModule()`;
- resamples and converts float input to PCM16 inside the processor;
- emits 20 ms target-rate chunks rather than assuming an input quantum size;
- keeps waveform analysis as a read-only tap on the capture graph; and
- handles native-spool and realtime-provider policy in the renderer runtime.

### 2. Transfer ownership, then apply two independent bounds

HTML transfer-list semantics permit ownership of an `ArrayBuffer` to move
instead of retaining duplicate live buffers. Nexa posts each PCM buffer with a
transfer list and reconstructs only a lightweight `Uint8Array` view in the
renderer.

Transfer alone is not backpressure. The worklet therefore has four explicit
in-flight credits plus at most eight pending chunks. The renderer ACKs every
delivered message. The pre-existing native and realtime upload queues remain
independently byte-, chunk-, and duration-bounded. A rejection or worklet
overflow is terminal for capture and triggers safe finalization of the accepted
native prefix.

The Google ring-buffer example validates the architectural pattern of adapting
render quanta into an application-sized buffer. Nexa uses a small purpose-built
queue rather than importing its WASM/ring-buffer implementation.

### 3. Pause and stop are explicit flush barriers

Pause first asks the worklet to flush the current partial segment and waits for
all transferred chunks to be ACKed; only then is the `AudioContext` suspended.
Resume restarts the same resampling phase. Stop changes visible UI state
immediately, then waits behind a hard 250 ms worklet flush timeout before native
spool finalization. This keeps UI feedback under the 300 ms acceptance bound
without allowing an unbounded promise chain.

The microphone track's `ended` event is a terminal capture signal. Nexa marks
the transport disconnected and safely finalizes rather than attempting to
silently continue. Cleanup disconnects the graph, closes the context, clears
handlers, and stops all tracks so capture resources and privacy indicators do
not outlive the session.

### 4. The native spool remains the durable authority

`MediaRecorder` can emit Blob events, but using it as the primary boundary
would reintroduce container/Blob accumulation and provider-specific decoding
into the renderer. Nexa needs ordered mono PCM16 for both local and realtime
STT, so it keeps the existing native opaque-handle spool as the only durable
recording authority.

Realtime transcription receives the same bounded chunks as a secondary
consumer. If its queue rejects, its request fails, or the connection degrades,
the Dock exposes the degraded state and transcription falls back to the native
spool. Cancel remains different from failure: it explicitly deletes both the
provider session and private native audio.

### 5. Recording is a stateful Dock, not an overloaded icon

LiveKit groups microphone control, visualization, and device choice because
they describe one active media session. Nexa adopts that information
architecture, while retaining its own provider, privacy, and transcription
states:

- collapsed baseline of 56 px with duration, waveform, provider/language,
  transport status, pause/resume, stop, and delete;
- expandable microphone/provider/language/storage detail;
- visible realtime partial transcript;
- processing state that remains mounted until the final transcript resolves;
- a labeled region, polite atomic status, and reduced-motion-safe animation;
  and
- local device labels refreshed after microphone permission is granted.

At narrow widths the Dock becomes the first full-width row in the composer.
CSS flex items default to an automatic minimum size, so every `ChatMessages`
render branch explicitly uses `min-h-0` plus vertical scrolling, while the
composer is `shrink-0`. The message area now yields height and the controls stay
inside the viewport.

## Rejected approaches

| Approach | Reason rejected |
| --- | --- |
| `ScriptProcessorNode` | Main-thread callback and deprecated architecture; fails the renderer-jank and memory goals. |
| Full-recording arrays, WAV encoding, or Blob accumulation in React | Makes renderer memory scale with recording duration and delays stop. |
| Uncredited `MessagePort.postMessage()` | Transfer avoids copying but does not cap producer/consumer distance. |
| `SharedArrayBuffer` ring buffer | Adds cross-origin-isolation and synchronization obligations that Nexa does not need for 20 ms STT chunks. |
| Provider-only realtime recording | A provider outage would lose the only copy and violate local fallback/privacy recovery. |
| Copying LiveKit or Google sample code | Their runtime and UI contracts differ; only the documented seams and interaction patterns were adopted. |

## Implemented verification

- `run-voice-runtime-contracts.mjs` executes the worklet in an isolated VM and
  checks fixed PCM16 chunks, transfer lists, credits, bounded terminal
  overflow, pause flush, and resampling phase across render quanta.
- `chat-recording-dock.spec.ts` covers live partials, pause/resume, detail
  disclosure, a minimum 420 px desktop Dock, full-width narrow layout,
  in-viewport controls, stop feedback under 300 ms, processing persistence,
  final transcript insertion, and explicit cancel/delete.
- Production build verification requires a separately emitted
  `voicePcmProcessor-*.js` asset.

## Reproducible revision audit

The snapshots were obtained with `git ls-remote <repository> HEAD`. A reviewer
can re-run that command to detect upstream movement; the evidence in this note
will remain reproducible because every source and license link above contains
the recorded 40-character revision.
