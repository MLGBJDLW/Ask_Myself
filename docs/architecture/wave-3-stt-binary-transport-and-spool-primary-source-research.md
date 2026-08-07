# Wave 3 STT Binary Transport and Native Spool: Primary-source Research

This note records the primary-source review for PR 11,
`stt-binary-transport-and-spool`, from the frozen STT requirements and Wave 3
roadmap in `D:\Nexa.txt` lines 465-591, 1274-1285, 1310-1319, and 1350-1365.
It was prepared on 2026-08-07 against immutable upstream commits. It is a
design input for an independent Nexa implementation. No upstream source is
copied into Nexa.

## Executive decision

PR 11 should complete the bounded renderer-to-native data path and make a
recoverable native spool the source of truth for recorded audio. It should not
attempt the AudioWorklet or Recording Dock work reserved for PR 12:

1. Start one native `VoiceSpoolSession` before capture. Both batch and realtime
   recording send fixed, mono PCM16 chunks as Tauri raw binary bodies. The
   renderer must no longer retain, merge, resample, or WAV-encode a complete
   recording.
2. The native session validates format, sequence, chunk size, total duration,
   and byte limits, then appends to one WAV writer owned by a single actor.
   Tauri headers carry only small routing metadata; the raw body carries only
   audio bytes.
3. The spool is authoritative. Realtime provider delivery is a second bounded
   consumer. A slow or unavailable provider may become `degraded` while native
   recording continues; it must not create an unbounded promise/channel chain
   or cause already accepted audio to disappear.
4. Backpressure is enforced at renderer queue, Tauri command, native actor, and
   provider-delivery boundaries by chunk count, bytes, and buffered duration.
   If native disk persistence cannot keep up, Nexa pauses or safely stops with
   an explicit error. It does not silently drop archival audio.
5. Hound `flush()` provides periodic readable checkpoints and `finalize()` is
   mandatory on normal stop. A destructor is cleanup insurance only; its
   ignored errors are never success evidence. Durability additionally requires
   an explicit file sync and a committed session-state transition.
6. The renderer receives an opaque spool handle plus format, duration, length,
   and checksum metadata. Absolute native paths never cross IPC. Local and
   cloud STT adapters open or stream the spool in native code with bounded
   memory.
7. Spool lifecycle is durable and privacy-visible: active, checkpointed,
   finalizing, ready, transcribing, recoverable, deletion-pending, and deleted
   are explicit states. Startup reconciliation recovers or expires orphaned
   sessions, and deletion failures remain visible and retryable.

This boundary is necessary for the frozen acceptance line: renderer memory may
not grow linearly with recording duration, Stop must update the UI within
300 ms, there must be no unbounded promise queue, 30/60-minute soak tests must
survive, and temporary audio must have an explicit privacy lifecycle.

## Reviewed upstream revisions

| Project | Immutable revision | Evidence used |
| --- | --- | --- |
| Tauri | `tauri` 2.11.5 and JavaScript API 2.11.1 at [`7cd71369c00978a3783b6ae3e9972358abbe4ae6`](https://github.com/tauri-apps/tauri/commit/7cd71369c00978a3783b6ae3e9972358abbe4ae6) | `Uint8Array`/`ArrayBuffer` invoke bodies, request headers, Rust raw-body representation, raw-response support, and raw-vs-JSON argument boundary |
| whisper.cpp | Current source at [`306c88f4d1286aec1bf96e544632897886af5501`](https://github.com/ggml-org/whisper.cpp/commit/306c88f4d1286aec1bf96e544632897886af5501) | Fixed-duration capture ring, bounded inference windows, overlap retention, and explicit overload/drop behavior for live recognition |
| Hound | 3.5.1 at [`2cddb275183a6146c0dff2c758ff936d00147af1`](https://github.com/ruuda/hound/commit/2cddb275183a6146c0dff2c758ff936d00147af1) | Streaming WAV writes, header checkpoints, finalize/error semantics, RIFF length bounds, and destructor limitations |
| tempfile | 3.27.0 at [`5c8fa12eb584931b4f1bccfde87eb72fbfa7dc61`](https://github.com/Stebalien/tempfile/commit/5c8fa12eb584931b4f1bccfde87eb72fbfa7dc61) | Private randomized files, named-path and cleanup hazards, explicit close/delete errors, app-selected backing directories, and memory-to-disk spooling |
| Rust standard library | Versioned 1.88 documentation | `create_new`, `sync_all`, rename, and file-deletion contracts used by Nexa's minimum supported Rust toolchain |
| SQLite | Official atomic-commit documentation | Durable recovery ordering: sync recovery state before primary mutation, sync primary state before removing the recovery marker, and never blindly delete uncertain recovery artifacts |

The Tauri, Hound, and tempfile versions match this repository's current
manifests/lockfile. whisper.cpp is design evidence only: Nexa's local Whisper
runtime is implemented with Candle, so PR 11 must not add whisper.cpp as a new
runtime dependency.

## Requirements traced from `D:\Nexa.txt`

| Frozen requirement | PR 11 exit evidence |
| --- | --- |
| Remove `Array.from(audioBytes)` and JSON number-array IPC | Batch and realtime append commands accept only bounded `InvokeBody::Raw`; regression tests reject JSON audio arrays |
| Fixed chunks; no complete PCM in renderer | Both recording modes set `captureWav: false`; each callback emits a bounded PCM16 chunk and releases it after the native acknowledgement |
| Native Rust or Worker spool | One native spool actor owns the file, WAV state, queue, limits, and lifecycle; no renderer path or full audio copy is authoritative |
| Stop passes handle, encoding, duration, rate, checksum | Finalization returns an opaque handle and validated metadata; native consumers resolve it inside the managed root |
| Bounded realtime queue and local degradation | Renderer and native queues expose count/bytes/duration limits; provider lag changes delivery state while capture continues to native disk |
| Provider unavailable: save and transcribe later | A finalized/recoverable spool remains available to the same configured adapter or an explicitly permitted fallback; no silent cross-provider egress |
| Disk-space and cancel behavior | Quota/preflight/write errors are classified; Cancel closes handles and explicitly deletes, or records deletion-pending for retry |
| Long-recording acceptance | 1/5/15/30/60-minute tests prove bounded renderer/native buffers, responsive Stop, restart recovery, and deterministic cleanup |
| PR split | AudioWorklet, device UX, pause/resume UI, partial-transcript dock, and waveform redesign remain PR 12 |

## Reviewed Nexa seams

The branch already contains the Wave 0 binary-IPC first stage, but it is not a
native spool architecture yet:

- [`useVoiceRecorder.ts`](../../apps/desktop/src/features/voice/useVoiceRecorder.ts)
  still uses `ScriptProcessorNode`. Batch mode appends every `Float32Array` to
  `buffersRef`, copies them into one merged allocation on Stop, resamples the
  whole recording with `OfflineAudioContext`, and synchronously writes every
  WAV sample in the renderer. Realtime mode avoids this only because it sets
  `captureWav: false`.
- [`boundedAudioQueue.ts`](../../apps/desktop/src/features/voice/boundedAudioQueue.ts)
  is a useful renderer guard: its queue is sequential and bounded by chunks,
  bytes, and chunk size. On overflow or send failure it becomes terminal and
  discards the queue. It has no native-spool fallback and does not measure
  buffered audio duration.
- [`voiceInputRuntime.ts`](../../apps/desktop/src/features/voice/voiceInputRuntime.ts)
  already switches realtime capture to 24 kHz PCM16 and sends through the
  bounded queue. Rejection invokes safe Stop. Batch capture still returns a
  complete WAV, and realtime provider failure currently cancels the provider
  session rather than preserving one recoverable recording job.
- [`api.ts`](../../apps/desktop/src/lib/api.ts) already passes `Uint8Array`
  directly to `invoke`. Realtime uses a request header for its session ID. This
  is the correct Tauri transport shape and should be generalized to spool
  sessions, not replaced with base64 or number arrays.
- [`commands/media.rs`](../../apps/desktop/src-tauri/src/commands/media.rs)
  accepts only raw audio and caps the one-shot WAV at 64 MiB, but clones the
  complete native body. For local backends it writes the full body to
  `%TEMP%/nexa-voice/voice-<uuid>.wav`; cleanup is a best-effort `Drop` that
  ignores deletion errors. There is no manifest, checkpoint, restart recovery,
  directory-permission hardening, quota, or retention policy.
- [`commands/realtime_transcription.rs`](../../apps/desktop/src-tauri/src/commands/realtime_transcription.rs)
  caps each raw PCM chunk at 256 KiB and uses a 64-item Tokio channel. That
  bounds count but permits roughly 16 MiB before actor-local/base64 copies, has
  no duration budget, and sends each provider payload only after base64 JSON
  expansion. A socket failure terminates the session; no spool can replay it.
- [`speech_to_text.rs`](../../crates/core/src/speech_to_text.rs) accepts complete
  `Vec<u8>` WAV values. OpenAI-compatible multipart owns the full bytes, and
  DashScope creates a whole-file base64 data URL plus JSON body. Those native
  peaks remain linear even after renderer IPC is fixed.
- [`video.rs`](../../crates/core/src/video.rs) already uses Hound and a temporary
  directory for bounded cloud-media chunks. Its local Candle Whisper path,
  however, reads the complete WAV into `Vec<f32>` and computes the full mel
  spectrogram before 30-second decode windows. PR 11 cannot claim 60-minute
  bounded-memory support while microphone transcription still takes this path.
- `hound` 3.5.1 and `tempfile` 3.27.0 are already dependencies. PR 11 should
  deepen those existing seams rather than add an unrelated native audio stack.

The new spool must remain feature-safe. Voice/media symbols currently cross the
`video` feature boundary, so Rust checks must cover both default and `video`
configurations even if no product behavior changes in the default build.

## 1. Tauri raw IPC is a bounded transport, not a file store

### Primary-source evidence

Tauri's JavaScript API accepts a record, number array, `ArrayBuffer`, or
`Uint8Array` as invoke arguments and separately accepts headers
([invoke contract](https://github.com/tauri-apps/tauri/blob/7cd71369c00978a3783b6ae3e9972358abbe4ae6/packages/api/src/core.ts#L222-L256)).
Its IPC conversion script emits `application/octet-stream` only when the
top-level value is an `ArrayBuffer`, view, or array; ordinary objects are JSON
serialized
([payload conversion](https://github.com/tauri-apps/tauri/blob/7cd71369c00978a3783b6ae3e9972358abbe4ae6/crates/tauri/scripts/process-ipc-message-fn.js#L5-L40)).
Therefore `{ audioData: uint8Array, metadata: ... }` is not an acceptable
binary envelope: it returns to JSON number-array expansion.
On the Rust side a byte payload is an owned `InvokeBody::Raw(Vec<u8>)`
([body representation](https://github.com/tauri-apps/tauri/blob/7cd71369c00978a3783b6ae3e9972358abbe4ae6/crates/tauri/src/ipc/mod.rs#L52-L93)).
`tauri::ipc::Request` exposes borrowed body and headers to a command
([request contract](https://github.com/tauri-apps/tauri/blob/7cd71369c00978a3783b6ae3e9972358abbe4ae6/crates/tauri/src/ipc/mod.rs#L143-L172)).

Raw bodies and normal deserialized command arguments are different modes. When
a command argument asks the generic deserializer to read a raw invocation,
Tauri returns an error instead of extracting JSON keys
([deserialization boundary](https://github.com/tauri-apps/tauri/blob/7cd71369c00978a3783b6ae3e9972358abbe4ae6/crates/tauri/src/ipc/command.rs#L83-L101)).
This supports Nexa's current choice to place a small session identifier in a
header while keeping audio in the body.

Tauri also represents raw responses explicitly
([response bodies](https://github.com/tauri-apps/tauri/blob/7cd71369c00978a3783b6ae3e9972358abbe4ae6/crates/tauri/src/ipc/mod.rs#L96-L139)),
but PR 11 does not need to return recorded bytes to the renderer.

### Nexa transport contract

Use a small JSON control plane and a raw binary data plane:

```text
start_voice_spool({ purpose, sampleRate, channels, sampleFormat })
  -> { recordingId, maxChunkBytes, maxBufferedBytes, maxBufferedMs }

append_voice_spool(raw PCM16 body)
  headers:
    x-nexa-recording-id: UUID
    x-nexa-audio-sequence: monotonically increasing u64
  Tauri transport content-type: application/octet-stream
  -> { acceptedSequence } or typed error

finalize_voice_spool({ recordingId })
  -> { spoolHandle, encoding, sampleRate, channels, durationMs,
       pcmBytes, fileBytes, checksum, recoveryState }

cancel_voice_spool({ recordingId })
  -> { deletionState }
```

The Rust request body is borrowed, so today's `audio_data.clone()` is a native
copy. PR 11 need not claim zero-copy. It must make that copy fixed and bounded,
move it immediately to the spool actor, and remove the complete-WAV invocation.
“Binary IPC” means no JSON object inflation and bounded copies, not magical
shared memory.

Each session is bound to the creating webview/window and one audio format.
Caller-supplied headers are untrusted routing metadata; Tauri assigns the raw
transport content type itself. Headers are parsed with strict length and syntax
limits. The recording UUID is
an opaque router, not authentication and not a filesystem name supplied by the
renderer. Sequence
numbers make retries idempotent: the same sequence and digest is accepted once,
a conflicting duplicate or gap is rejected, and finalization waits only for the
last acknowledged sequence.

The reviewed Tauri source explicitly states that raw invoke bodies are not
supported on Android
([platform boundary](https://github.com/tauri-apps/tauri/blob/7cd71369c00978a3783b6ae3e9972358abbe4ae6/crates/tauri/src/ipc/mod.rs#L52-L63)).
Nexa is a desktop product, so PR 11 may use this transport, but shared code and
documentation must not advertise it as a mobile-compatible protocol.

## 2. Bound every queue by count, bytes, and time

### Primary-source evidence

whisper.cpp's streaming example allocates capture storage from a fixed
millisecond window rather than recording duration
([fixed capture capacity](https://github.com/ggml-org/whisper.cpp/blob/306c88f4d1286aec1bf96e544632897886af5501/examples/common-sdl.cpp#L35-L78)).
The file itself labels the program a quick proof of concept
([scope warning](https://github.com/ggml-org/whisper.cpp/blob/306c88f4d1286aec1bf96e544632897886af5501/examples/stream/stream.cpp#L1-L3)),
so it is evidence for bounds, not a production concurrency blueprint.
Its callback writes into a ring and caps stored length at ring capacity
([ring update](https://github.com/ggml-org/whisper.cpp/blob/306c88f4d1286aec1bf96e544632897886af5501/examples/common-sdl.cpp#L138-L167));
reads copy at most the requested window
([bounded read](https://github.com/ggml-org/whisper.cpp/blob/306c88f4d1286aec1bf96e544632897886af5501/examples/common-sdl.cpp#L170-L210)).

The stream loop defines step, total analysis window, and retained overlap in
milliseconds
([window derivation](https://github.com/ggml-org/whisper.cpp/blob/306c88f4d1286aec1bf96e544632897886af5501/examples/stream/stream.cpp#L124-L175)).
When recognition falls more than two steps behind it warns, clears the live
buffer, and continues instead of allowing unlimited growth
([overload policy](https://github.com/ggml-org/whisper.cpp/blob/306c88f4d1286aec1bf96e544632897886af5501/examples/stream/stream.cpp#L251-L290)).
It retains only a small overlap to reduce word-boundary loss
([overlap retention](https://github.com/ggml-org/whisper.cpp/blob/306c88f4d1286aec1bf96e544632897886af5501/examples/stream/stream.cpp#L405-L423)).

### What Nexa may and may not borrow

Nexa should borrow the fixed-duration capacity, sliding inference windows,
small overlap, and explicit overload state. It must not copy the example's
audio-drop policy into the authoritative recording path. Dropping stale audio
is acceptable for a disposable live preview; it is not acceptable for a file
the user expects to transcribe later. It also must not copy the example's
separate `get()` and later `clear()` operations: callback samples can arrive
between those locks. Nexa needs one atomic drain/cursor transition or a bounded
single-owner channel.

Use distinct bounded stages:

| Stage | Required bounds | Overflow behavior |
| --- | --- | --- |
| Renderer upload queue | chunks, bytes, PCM milliseconds, max chunk | stop accepting capture callbacks, surface backpressure, request native safe Stop |
| Native spool actor | channel items, owned bytes, PCM milliseconds | reject before ownership growth; continue only if the file writer can drain |
| Provider live-delivery queue | chunks, bytes, audio lag | mark provider degraded and stop/reconnect delivery; native spool remains active |
| Local/cloud batch worker | one decode/upload window plus overlap | checkpoint progress; never materialize the complete recording |

A Tokio channel capacity is only a count bound. The native actor also needs an
acquisition budget for bytes and duration before a command is enqueued. The
budget is released only after the writer commits that chunk. Fixed chunk
duration is part of the format contract so telemetry can report real audio lag,
not only object count.

Provider delivery must consume only already-spooled sequences. If it falls
behind, the actor records the last delivered sequence and can replay or submit
the finalized file only when the selected adapter supports that behavior.
Cross-provider retry remains governed by Capability Binding and consent; a
network failure never silently sends audio to another vendor.

## 3. The native spool is a state machine

One actor owns each writer so append/finalize/cancel cannot race:

```text
Created -> Recording -> Finalizing -> Ready -> Transcribing -> Completed
              |              |          |             |
              +----------> CancelRequested ----------+
                               |
                          DeletionPending -> Deleted

process restart:
Recording/Finalizing -> Recoverable | Corrupt | Expired
Ready/Transcribing   -> Ready       | Recoverable | Expired
```

The provider stream has an independent substate:

```text
NotRequested | Connecting | Live | Degraded | Replaying | Final | Failed
```

This separation prevents a provider socket failure from changing capture
durability. State transitions are idempotent and revisioned. Finalize is
exactly-once; concurrent Cancel wins only after the writer stops accepting
chunks. Commands return stable typed errors such as `unknown_session`,
`wrong_owner`, `sequence_gap`, `chunk_too_large`, `format_mismatch`,
`spool_quota_exceeded`, `disk_full`, `already_finalizing`, and
`deletion_pending`.

Persist a secret-free session row with:

- recording ID, schema version, revision, purpose, created/updated timestamps;
- relative spool path under one managed root, never an arbitrary IPC path;
- PCM format, next sequence, accepted frames/bytes, duration;
- last checkpoint sequence/frames/time and rolling PCM digest state or digest;
- final WAV length and checksum when available;
- lifecycle, provider-delivery substate, retry count, and redacted error class;
- retention deadline and deletion attempts.

File state leads database state. At a checkpoint: update the WAV header, flush,
sync the file, then transactionally advance the recorded checkpoint. At final
Stop: stop appends, drain acknowledged chunks, explicitly finalize, call
`sync_all`, publish within the same filesystem, sync the containing directory
where the platform supports it, validate the WAV, calculate its checksum by
bounded native streaming, then mark it Ready.
If the process dies between steps, startup reconciliation chooses the last
verified file/database pair rather than guessing from a filename.

This follows SQLite's transferable durability ordering: make recovery data
durable before changing primary state, make primary state durable before
removing the recovery marker, and do not delete a potentially active recovery
artifact merely because its name looks stale
([atomic commit and recovery](https://www.sqlite.org/atomiccommit.html#_flushing_the_rollback_journal_file_to_mass_storage),
[hot-journal warning](https://www.sqlite.org/atomiccommit.html#_deleting_or_renaming_a_hot_journal)).
Nexa should borrow the ordering principle, not SQLite's journal format or code.

The Stop click updates UI state to `processing` before awaiting finalization.
The 300 ms acceptance line applies to visible response, not to completing a
full checksum or provider transcription on the renderer thread.

## 4. Hound checkpoint, finalize, and error semantics

### Primary-source evidence

Hound requires interleaved samples to end on a complete channel frame and says
the writer must be finalized. Drop attempts finalization but cannot report a
failure
([writer contract](https://github.com/ruuda/hound/blob/2cddb275183a6146c0dff2c758ff936d00147af1/src/write.rs#L154-L182)).
The writer tracks the data length as `u32`, reflecting the classic RIFF/WAVE
size boundary
([writer state](https://github.com/ruuda/hound/blob/2cddb275183a6146c0dff2c758ff936d00147af1/src/write.rs#L160-L178)).

`flush()` rewrites the RIFF/data lengths and flushes the underlying writer so a
compliant decoder can read through the last checkpoint
([checkpoint behavior](https://github.com/ruuda/hound/blob/2cddb275183a6146c0dff2c758ff936d00147af1/src/write.rs#L488-L532)).
It still reports `UnfinishedSample` when sample count is not a multiple of
channels. `finalize()` additionally performs an explicit flush so buffered
write errors are observable
([finalize behavior](https://github.com/ruuda/hound/blob/2cddb275183a6146c0dff2c758ff936d00147af1/src/write.rs#L535-L548)).
Drop ignores header-update failures
([drop behavior](https://github.com/ruuda/hound/blob/2cddb275183a6146c0dff2c758ff936d00147af1/src/write.rs#L579-L589)).

Hound's `create()` convenience opens a buffered file but overwrites an existing
path
([create behavior](https://github.com/ruuda/hound/blob/2cddb275183a6146c0dff2c758ff936d00147af1/src/write.rs#L639-L650)).
Nexa must instead securely create a unique file handle first, then pass that
handle to `WavWriter::new`; it must never trust a generated-looking path and
call an overwrite constructor.

### Nexa WAV constraints

- One session freezes mono PCM16 little-endian and one target sample rate.
  Every raw chunk must have complete 16-bit samples and complete channel frames.
- Batch uses incremental 16 kHz encoding; realtime uses the adapter's declared
  rate (currently 24 kHz). Native code writes the declared rate into the WAV.
- Use a buffered writer and retain a cloned file handle solely for durability
  sync. Hound flushes userspace buffers; it does not itself guarantee stable
  storage. Rust's versioned [`File::sync_all`](https://doc.rust-lang.org/1.88.0/std/fs/struct.File.html#method.sync_all)
  contract is the additional file durability primitive; publishing a new name
  also requires containing-directory synchronization where supported.
- Checkpoint by bounded time or bytes. Recovery loss may be no greater than one
  declared checkpoint interval, and the UI must disclose any truncated tail.
- On crash recovery, trust only the header-declared complete frames from the
  last checkpoint. Salvage into a newly created finalized WAV by bounded read
  and write; do not append after unverified trailing bytes.
- Treat `IoError`, `UnfinishedSample`, `TooWide`, `Unsupported`, and
  `InvalidSampleFormat` as distinct failure classes
  ([Hound errors](https://github.com/ruuda/hound/blob/2cddb275183a6146c0dff2c758ff936d00147af1/src/lib.rs#L364-L415)).
- Enforce a maximum duration and byte count below Hound's `u32` data boundary.
  The required 60-minute mono PCM16 recordings are well inside that bound, but
  the invariant must be explicit rather than relying on overflow behavior.

## 5. Secure file creation, recovery, and deletion

### Primary-source evidence

tempfile distinguishes unnamed files, named files, temporary directories, and
memory-to-disk spooled files
([type selection](https://github.com/Stebalien/tempfile/blob/5c8fa12eb584931b4f1bccfde87eb72fbfa7dc61/src/lib.rs#L1-L16)).
Named resources depend on destructors, which may not run on signals or abnormal
exit, and long-lived named files in a cleaner-managed system temp directory can
be removed and replaced
([leak and cleaner hazards](https://github.com/Stebalien/tempfile/blob/5c8fa12eb584931b4f1bccfde87eb72fbfa7dc61/src/lib.rs#L18-L60)).
Temporary files are private by default, but temporary directories use default
permissions and can be world-readable depending on platform/umask
([permission boundary](https://github.com/Stebalien/tempfile/blob/5c8fa12eb584931b4f1bccfde87eb72fbfa7dc61/src/lib.rs#L62-L67)).

`TempPath::close()` and `NamedTempFile::close()` expose deletion failures that
Drop would hide
([explicit file removal](https://github.com/Stebalien/tempfile/blob/5c8fa12eb584931b4f1bccfde87eb72fbfa7dc61/src/file/mod.rs#L122-L165),
[named-file close](https://github.com/Stebalien/tempfile/blob/5c8fa12eb584931b4f1bccfde87eb72fbfa7dc61/src/file/mod.rs#L704-L730)).
Persisting a named temporary file does not synchronize its contents or parent
directory
([persist boundary](https://github.com/Stebalien/tempfile/blob/5c8fa12eb584931b4f1bccfde87eb72fbfa7dc61/src/file/mod.rs#L732-L747)).
Likewise, `TempDir` Drop silently ignores recursive-deletion errors and cannot
run after a crash
([directory lifecycle](https://github.com/Stebalien/tempfile/blob/5c8fa12eb584931b4f1bccfde87eb72fbfa7dc61/src/dir/mod.rs#L113-L142),
[explicit close](https://github.com/Stebalien/tempfile/blob/5c8fa12eb584931b4f1bccfde87eb72fbfa7dc61/src/dir/mod.rs#L433-L501)).

The crate's `SpooledTempFile` demonstrates a threshold-triggered switch from an
in-memory cursor to an unnamed disk file
([spooled states](https://github.com/Stebalien/tempfile/blob/5c8fa12eb584931b4f1bccfde87eb72fbfa7dc61/src/spooled.rs#L7-L25),
[rollover](https://github.com/Stebalien/tempfile/blob/5c8fa12eb584931b4f1bccfde87eb72fbfa7dc61/src/spooled.rs#L94-L158)).
That is a useful threshold pattern but not a complete Nexa recovery solution:
the unnamed file has no stable child-process/restart handle, and it begins in
memory, while the frozen requirement says long recordings should append to disk.

### Nexa storage boundary

- Store spools under a dedicated per-user application-data root, not
  `std::env::temp_dir()/nexa-voice`. The root must be outside OS cleaner policy
  and owner-only where the platform permits.
- Create session files securely with randomized exclusive creation in that
  root. Rust's versioned
  [`OpenOptions::create_new`](https://doc.rust-lang.org/1.88.0/std/fs/struct.OpenOptions.html#method.create_new)
  performs atomic create-or-fail; never check `exists()` and then create.
- Stage and publish on the same filesystem. Rust's versioned
  [`rename`](https://doc.rust-lang.org/1.88.0/std/fs/fn.rename.html) contract
  does not cross mount points, and tempfile's persist operation explicitly does
  not sync file contents or the containing directory. Finalize and `sync_all`
  before publish, handle `PersistError` as recoverable state, then sync the
  parent directory where the platform exposes that operation.
- Persist only a relative generated filename. On every open/delete, join it to
  the canonical managed root and reject traversal, symlinks/reparse-point
  escapes, unexpected file type, and identifiers not owned by a database row.
- Keep capture files and metadata free of provider keys, transcript text, user
  prompts, and original document paths. Recording IDs in logs are truncated or
  hashed; raw audio and absolute paths never enter telemetry.
- Maintain a per-session and global spool quota. Check available space at start
  as advisory only; every write remains fallible and disk-full is a terminal
  capture condition with a recoverable partial file.
- Normal Cancel and successful post-transcription cleanup explicitly close all
  handles and delete. A Windows sharing violation or other error records
  `DeletionPending`; a background/startup sweeper retries with bounded backoff.
  Nexa must not report “deleted” until the filesystem operation succeeds or the
  file is confirmed absent.
- A visible retention policy bounds recoverable audio. A privacy-first default
  may delete after successful transcription and retain interrupted sessions for
  a short recovery window, but the exact window must be product-configurable
  and shown to the user rather than hidden in code.

At startup, reconcile database rows and the managed directory in both
directions. Known active files become Ready/Recoverable/Corrupt/Expired based on
header, length, and checkpoint metadata. Unknown files are quarantined by name
and type and removed only under the same bounded cleanup policy. Rows with
missing files become explicit loss records; they are not silently treated as
empty transcripts.

## 6. Provider and local-runtime integration boundary

Moving capture to disk is insufficient if the next layer reads the whole file
back into memory. PR 11 must provide path/reader-based STT entry points:

- **OpenAI-compatible multipart:** stream the finalized file or bounded WAV
  segments from native code. Do not construct a complete `Part::bytes(Vec)` for
  a 60-minute recording.
- **DashScope ASR:** its current data-URL request necessarily expands bytes to
  base64 and then JSON. Segment the finalized WAV natively to provider-supported
  bounded units and encode one unit at a time. Tauri binary IPC does not make
  this provider wire format binary.
- **Realtime WebSocket:** base64 remains the provider protocol for each bounded
  chunk. The provider actor may reconnect/replay only if the adapter contract
  says it is safe; sequence and partial transcript semantics must not be
  invented. The spool remains available for later batch transcription.
- **sherpa-onnx:** retain the path-based subprocess interface, but resolve only
  managed spool handles and keep the deletion guard alive until the process
  exits.
- **Candle Whisper:** replace whole-file `read_wav_pcm -> Vec<f32> -> full mel`
  with fixed audio windows and a small overlap. Reuse the cached model/runtime,
  but bound per-window PCM, mel, token, and transcript state. whisper.cpp's
  overlap is a design precedent, not code to transplant.

Provider unavailability does not authorize a different provider. The session
records the originally resolved STT capability target and revision. Retry uses
that target; cross-provider or cloud/local fallback occurs only when the user's
Capability Binding and data-egress policy explicitly allow it.

## 7. Privacy, integrity, and abuse boundaries

| Threat/failure | Required control |
| --- | --- |
| Renderer sends unbounded or malformed audio | Raw-only body, fixed max chunk, PCM frame alignment, strict session format, count/byte/time quotas |
| Cross-session injection or stale callback | Bind recording ID to creator, strict monotonically increasing sequence, idempotent same-digest retry, reject gaps/conflicts |
| Memory growth moves from JS to Rust | Byte semaphore plus bounded actor queues; fixed inference/upload windows; no complete `Vec`/base64/mel for long recordings |
| Disk exhaustion | Per-session/global quotas, write-error classification, safe Stop, retained partial checkpoint, visible cleanup |
| Predictable or replaced temp path | App-private root, random exclusive create, relative managed identity, canonical-root validation, no renderer paths |
| Crash leaves private audio indefinitely | Durable session rows, startup reconciliation, visible retention deadline, explicit sweeper and deletion status |
| Drop/finalize/delete error is hidden | Explicit Hound finalize, file sync, validation, explicit close/remove result, deletion-pending retry |
| WAV header is stale after crash | Periodic Hound checkpoint plus sync; recover only header-declared complete frames into a new finalized file |
| Silent provider fallback leaks speech | Pin target/provider revision; require explicit cross-provider/local-cloud consent on every retry/fallback |
| Log or IPC leaks sensitive data | Opaque handle only; no audio/path/key/transcript in queue telemetry or errors; bounded redacted provider bodies |
| App sleep/device loss/repeated Stop races | Single owner actor, revisioned terminal transitions, drain acknowledged sequence exactly once, idempotent finalize/cancel |

Checksums have two distinct purposes. A rolling digest over canonical PCM plus
format metadata identifies accepted audio independently of mutable WAV headers.
A final SHA-256 over the finalized WAV verifies the artifact. Neither checksum
is a secret, but it should not become a cross-workspace tracking identifier in
analytics.

## 8. Required tests and release gates

### Transport and queue tests

- Batch and realtime commands accept `Uint8Array` raw bodies and reject JSON
  number arrays, empty invalid chunks, oversized chunks, odd PCM16 lengths,
  wrong content type, malformed IDs, and unsupported format/rate/channel data.
- Sequence tests cover duplicate retry with same digest, conflicting duplicate,
  gap, out-of-order append, append-after-finalize, concurrent finalize/cancel,
  and events arriving from a stale recorder callback.
- Renderer and native tests prove limits by chunk count, bytes, and audio
  milliseconds. Native/provider stalls never exceed the configured memory
  envelope or create an unbounded Promise/Tokio queue.
- A provider stall degrades delivery while the native file continues growing.
  A native disk stall or write error safe-stops capture rather than dropping
  accepted audio.

### WAV and persistence tests

- Golden mono PCM16 chunks produce the expected sample rate, duration, byte
  length, RIFF/data lengths, PCM digest, and final WAV SHA-256.
- Hound errors for incomplete frames, invalid format, file-too-large, disk-full,
  flush failure, and finalize failure are surfaced and cannot mark Ready.
- Checkpoints are readable. Crash/fault injection before/after header update,
  flush, sync, database checkpoint, finalization, checksum, and Ready commit
  recovers at most the declared checkpoint-loss window.
- Secure creation cannot overwrite a pre-existing file. Traversal, symlink or
  reparse escape, wrong file type, unmanaged filename, and missing database row
  cannot be opened or deleted through a spool handle.
- Cancel, success cleanup, retention expiry, app restart, open-handle deletion
  failure, and repeated cleanup produce correct Deleted/DeletionPending state.

### Adapter and memory tests

- OpenAI multipart, DashScope base64, realtime WebSocket, sherpa path, and local
  Candle Whisper tests show one bounded audio unit in memory at a time.
- Local Whisper window/overlap fixtures preserve ordering and avoid duplicate or
  missing boundary text within the declared reconciliation policy.
- Provider disconnect/reconnect, rate limiting, progressively slower chunks,
  invalid partial events, and batch retry do not alter or delete the spool.
- Explicitly authorized fallback records provider/target change; unauthorized
  cross-provider fallback is blocked while the audio remains recoverable.

### Soak and lifecycle tests

- Automated 1/5/15/30/60-minute PCM generators verify renderer heap and native
  queue memory plateau while disk use grows linearly within quota.
- Stop changes visible state within 300 ms; finalization/checksum/transcription
  continues outside the renderer main thread.
- Cover minimize/restore, sleep/resume, microphone unplug, default-device
  change, permission revocation, rapid start/stop/cancel, app shutdown, forced
  process termination, restart recovery, and disk-full behavior.
- Run TypeScript unit/E2E contracts plus Rust format, tests, Clippy/checks in
  both default and `video` feature configurations.

Telemetry is aggregate and secret-free: current/peak renderer queued bytes,
native queued bytes, buffered audio milliseconds, acknowledged sequence,
checkpoint age, provider lag, write/finalize/delete error class, event-loop lag,
and state-transition latency. It never includes audio, transcript content,
absolute path, credential, or full recording checksum.

## 9. PR 11 exit boundary

PR 11 is complete only when:

1. no microphone recording path sends JSON number arrays or one complete WAV
   over Tauri IPC;
2. batch and realtime capture both write fixed PCM16 chunks to one bounded,
   durable native spool and release renderer chunks after acknowledgement;
3. provider slowness/failure cannot create an unbounded queue or destroy
   already accepted audio;
4. Hound checkpoints/finalization, file sync, checksum, recovery, quota,
   explicit deletion, and retention states are tested and user-observable;
5. local and cloud transcription consume the spool with bounded windows rather
   than reconstructing a complete recording in `Vec`, base64, or mel memory;
6. the 1/5/15/30/60-minute, network, sleep, device, permission, rapid-control,
   disk-full, cancel, crash, restart, and cleanup matrix passes; and
7. default-feature and `video`-feature Rust checks remain green with no P1
   privacy, data-loss, unbounded-memory, or path-boundary issue.

## Non-goals

- Replacing `ScriptProcessorNode` with AudioWorklet; that is PR 12. PR 11 may
  route its existing fixed callback chunks to native code but must not combine
  the capture rewrite into this PR.
- Building the Recording Dock, pause/resume UX, waveform redesign, device detail
  expander, or partial-transcript layout; those are PR 12.
- Adding whisper.cpp or SDL as a Nexa runtime dependency, or replacing the
  Candle Whisper model implementation.
- Changing STT provider selection, credentials, Capability Registry semantics,
  or silently adding a cross-provider fallback.
- Inventing resumable semantics for a provider protocol that does not define
  them.
- Persisting transcripts, prompts, or provider credentials in the audio spool.
- Shipping mobile raw IPC; the reviewed Tauri contract excludes Android.
- Using the system temp directory, Drop-only cleanup, or an unnamed
  `SpooledTempFile` as the durable restart-recovery design.
- Retaining completed audio indefinitely or deleting recoverable audio without
  the declared privacy policy and state transition.

## Borrow / do-not-copy matrix

| Source | Borrow | Do not copy or assume |
| --- | --- | --- |
| Tauri | Raw `Uint8Array` body, small headers, borrowed `Request`, raw-vs-JSON separation | Zero-copy claims, JSON args mixed into a raw body, Android raw-body support, internal IPC implementation |
| whisper.cpp | Fixed-time ring, bounded windows, overlap, explicit overload telemetry | SDL capture, C++ inference/runtime, wholesale stream loop, or live-drop policy for authoritative audio |
| Hound | Existing dependency, mono PCM16 streaming writer, readable checkpoints, explicit finalize/errors | Drop as success, overwrite-by-path `create`, unbounded RIFF length, or stable-storage claims from `flush` alone |
| tempfile | Secure randomized creation, private file defaults, explicit close, app-selected directory, threshold concept | System-temp long-term paths, destructor-only cleanup, unnamed spool as restart handle, or unsynced persist as durability |
| Rust std | Exclusive create, sync, relative managed paths, explicit filesystem errors | Cross-filesystem atomic rename, platform-identical delete behavior, or unchecked user-supplied paths |

## License and integration boundary

| Project | License at reviewed revision | Nexa integration boundary |
| --- | --- | --- |
| Tauri | [MIT OR Apache-2.0](https://github.com/tauri-apps/tauri/blob/7cd71369c00978a3783b6ae3e9972358abbe4ae6/LICENSE_MIT) ([Apache text](https://github.com/tauri-apps/tauri/blob/7cd71369c00978a3783b6ae3e9972358abbe4ae6/LICENSE_APACHE-2.0)) | Existing dependency; use its public JS/Rust IPC API, not internal source |
| whisper.cpp | [MIT](https://github.com/ggml-org/whisper.cpp/blob/306c88f4d1286aec1bf96e544632897886af5501/LICENSE) | Architectural comparison only; do not add or port its C++/SDL code in PR 11 |
| Hound | [Apache-2.0](https://github.com/ruuda/hound/blob/2cddb275183a6146c0dff2c758ff936d00147af1/license) | Existing Rust dependency; call its public writer/reader API and retain dependency notices |
| tempfile | [MIT OR Apache-2.0](https://github.com/Stebalien/tempfile/blob/5c8fa12eb584931b4f1bccfde87eb72fbfa7dc61/LICENSE-MIT) ([Apache text](https://github.com/Stebalien/tempfile/blob/5c8fa12eb584931b4f1bccfde87eb72fbfa7dc61/LICENSE-APACHE)) | Existing Rust dependency; use public creation/cleanup API and Nexa-owned lifecycle metadata |
| Rust standard library documentation | MIT OR Apache-2.0 project terms | API documentation only; no standard-library source is copied |
| SQLite | [Public domain](https://www.sqlite.org/copyright.html) | Durability/recovery ordering comparison only; no SQLite journal implementation is copied |

This note does not authorize copying source, tests, examples, or audio assets.
The Nexa implementation should remain Rust/TypeScript code built around the
existing Tauri, Hound, tempfile, Candle, and provider-adapter boundaries. Any
later close adaptation of upstream code requires separate notice and license
review.
