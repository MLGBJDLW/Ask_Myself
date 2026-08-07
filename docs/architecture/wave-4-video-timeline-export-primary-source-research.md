# Wave 4 Simple Timeline and Export: primary-source research

Status: research record for PR16 (`video-timeline-and-export`)

Verified: 2026-08-07

Scope: design evidence only; no upstream code is copied

## Decision summary

PR16 should add a deliberately small, durable editing surface: one ordered video
track, hard cuts, per-clip source ranges, preview, and an FFmpeg-backed export.
It should not become a general node editor or nonlinear editor.

The durable boundary is a versioned timeline plus an immutable export snapshot.
An export is a persisted, cancellable job graph:

```text
Validate -> Normalize clip 0..n -> Concatenate -> Verify -> Publish
```

Each normalization stage produces a reusable, fingerprinted intermediate with a
single output profile. Concatenation may use FFmpeg's concat demuxer only after
probe results prove that every intermediate is compatible. The final artifact is
verified before it is published atomically. A process exit, a progress message,
or the mere existence of a file is not sufficient evidence of completion.

This design adapts editorial concepts from OpenTimelineIO, playlist/range
behavior from MLT, FFmpeg's documented machine protocol and concat constraints,
and Kdenlive's process orchestration. Nexa should not add OpenTimelineIO, MLT, or
Kdenlive as runtime dependencies for this MVP.

## Acceptance boundary rechecked

The final MVP in `D:\Nexa.txt` requires:

- Compare & Timeline after variant selection, including sorting, preview, and
  export;
- a Simple Timeline rather than a Premiere-like editor;
- FFmpeg composition and export;
- a structured workflow that can later support `Concat` and `Export` nodes; and
- a Wave 4 exit condition that selected shots can be concatenated and exported.

The following are explicitly outside PR16:

- Extend Video, Edit Video, Motion Transfer, storyboard automation, and the
  Wave 5 multi-shot DAG editor;
- multitrack editing, transitions, keyframes, effects, arbitrary filters, and
  arbitrary user-supplied FFmpeg options;
- importing or exporting OTIO interchange files; and
- frame-accurate source editing that depends on unimplemented proxy/index or
  codec-aware smart-render machinery.

PR16 may keep forward-compatible IDs and lineage, but it must not surface or
silently implement these deferred features.

## Primary sources

All source links below are immutable GitHub blobs pinned to a full 40-character
commit SHA.

### FFmpeg

Snapshot: `5c395992f99feb47860e4cc99a0cea2009457870`

- The [FFmpeg command-line documentation](https://github.com/FFmpeg/FFmpeg/blob/5c395992f99feb47860e4cc99a0cea2009457870/doc/ffmpeg.texi)
  defines `-progress` as machine-readable `key=value` output ending each update
  with `progress=continue` or `progress=end`. It also defines `-stats_period` and
  `-nostdin`, which suit a background child process.
- The [concat demuxer documentation](https://github.com/FFmpeg/FFmpeg/blob/5c395992f99feb47860e4cc99a0cea2009457870/doc/demuxers.texi)
  requires matching streams and describes `inpoint`, exclusive `outpoint`,
  duration caveats, `safe`, and the narrow scope of `auto_convert`.
- The [concat demuxer implementation](https://github.com/FFmpeg/FFmpeg/blob/5c395992f99feb47860e4cc99a0cea2009457870/libavformat/concatdec.c)
  implements safe-filename checks, defaults safe mode on, and rejects invalid
  in/out ranges and duration overflow.
- The [FFmpeg front-end](https://github.com/FFmpeg/FFmpeg/blob/5c395992f99feb47860e4cc99a0cea2009457870/fftools/ffmpeg.c)
  handles termination signals, exposes an interrupt callback, prints reports,
  writes trailers, and returns conversion failure independently from progress.
- The [FFmpeg license guide](https://github.com/FFmpeg/FFmpeg/blob/5c395992f99feb47860e4cc99a0cea2009457870/LICENSE.md)
  says the default project is LGPL 2.1 or later, while enabling GPL components
  changes the resulting build's obligations; nonfree configurations have still
  different redistribution consequences.

Applicable conclusions:

- Treat `-progress pipe:1` as a parsing protocol, not as proof of success.
- Leave concat `safe=1` enabled and give it generated relative names only.
- Do not assume concat can reconcile mismatched codecs, stream layouts, frame
  rates, time bases, dimensions, pixel formats, or audio formats.
- Do not assume `inpoint`/`outpoint` plus stream copy is sample- or frame-exact
  for inter-frame codecs. Normalize by decoding and encoding when exact output
  boundaries matter.
- Record the exact FFmpeg binary version, configuration, checksum, and license
  provenance used for each export and for any bundled release.

### OpenTimelineIO

Snapshot: `0eebd211b2055f111e2c53d04b5581adc594c1fc`

- The [timeline structure tutorial](https://github.com/AcademySoftwareFoundation/OpenTimelineIO/blob/0eebd211b2055f111e2c53d04b5581adc594c1fc/docs/tutorials/otio-timeline-structure.md)
  models a simple cut list as a Timeline containing a Track whose ordered Clip
  children play end-to-end. A Clip's `source_range` expresses its trim.
- The [Item contract](https://github.com/AcademySoftwareFoundation/OpenTimelineIO/blob/0eebd211b2055f111e2c53d04b5581adc594c1fc/src/opentimelineio/item.h)
  distinguishes an explicitly authored source range from the available media
  range and exposes a trimmed range.
- The [Composition contract](https://github.com/AcademySoftwareFoundation/OpenTimelineIO/blob/0eebd211b2055f111e2c53d04b5581adc594c1fc/src/opentimelineio/composition.h)
  owns ordered children and indexed insertion/removal, making order a domain
  property rather than a presentation-only array.
- The [OTIO schema specification](https://github.com/AcademySoftwareFoundation/OpenTimelineIO/blob/0eebd211b2055f111e2c53d04b5581adc594c1fc/docs/tutorials/otio-file-format-specification.md)
  uses explicit schema names/versions and namespaced metadata.
- OTIO is distributed under the [Apache License 2.0](https://github.com/AcademySoftwareFoundation/OpenTimelineIO/blob/0eebd211b2055f111e2c53d04b5581adc594c1fc/LICENSE.txt).

Applicable conclusions:

- Model the MVP as one ordered sequence of clips with source ranges.
- Keep asset availability and authored trim distinct.
- Version the timeline schema and isolate implementation metadata.
- Add Nexa validation: OTIO intentionally permits a source range to extend
  outside an available range because downstream applications decide policy.

Boundary: OTIO is an editorial interchange model, not a renderer or export job
engine. Its concepts are useful; a new OTIO serialization/runtime dependency is
not justified for PR16.

### MLT

Snapshot: `06c4785f951c087c700de942362d1d1c68ffe500`

- The [MLT playlist implementation](https://github.com/mltframework/mlt/blob/06c4785f951c087c700de942362d1d1c68ffe500/src/framework/mlt_playlist.c)
  persists an ordered producer plus `frame_in`, `frame_out`, frame count, and
  repeat; it provides append, insert, remove, move, and resize operations.
- The [melt front-end](https://github.com/mltframework/mlt/blob/06c4785f951c087c700de942362d1d1c68ffe500/src/melt/melt.c)
  connects a producer graph to a consumer, observes termination signals, stops
  the consumer, and preserves interruption in the process result.
- MLT is distributed under [LGPL 2.1](https://github.com/mltframework/mlt/blob/06c4785f951c087c700de942362d1d1c68ffe500/COPYING).

Applicable conclusions:

- Reordering and resizing are mutations of stable clip records, not regeneration
  of anonymous UI cards.
- Pick one documented range convention. MLT's frame range is inclusive and its
  frame count is `out - in + 1`; Nexa should instead use start plus duration so
  there is no ambiguous inclusive/exclusive end at persistence boundaries.
- Cancellation must survive into the durable job result, not merely close a UI.

Boundary: MLT already supplies a full media framework. Pulling it in beside
FFmpeg would duplicate the MVP execution engine and packaging surface. Adapt the
playlist semantics, not the dependency or source code.

### Kdenlive

Snapshot: `507dff83e3a6f2c483b3b73d5b63c35e77cfb07a`

- The [Kdenlive render job](https://github.com/KDE/kdenlive/blob/507dff83e3a6f2c483b3b73d5b63c35e77cfb07a/renderer/renderjob.cpp)
  passes a program and argument list separately to `QProcess`, parses frame and
  percentage progress, maps two-pass progress, handles abort, and verifies
  process error/exit and output existence before reporting success.
- Kdenlive is distributed under the [GNU GPL](https://github.com/KDE/kdenlive/blob/507dff83e3a6f2c483b3b73d5b63c35e77cfb07a/COPYING).

Applicable conclusions:

- Spawn FFmpeg with a structured argv and keep progress parsing separate from
  exit/result validation.
- Model cancellation and multi-stage progress explicitly.

Boundary: this is behavior-level research only. No GPL Kdenlive code is copied.
Kdenlive also removes destination files during some abort paths and holds much
render state in a live process. Nexa should instead protect any pre-existing
destination, publish atomically, and persist recovery state.

## Proposed PR16 domain model

Names are illustrative; the important part is the ownership and invariants.

```rust
struct VideoTimeline {
    id: TimelineId,
    workflow_id: WorkflowId,
    revision: u64,
    output_profile: OutputProfile,
    clips: Vec<VideoTimelineClip>,
}

struct VideoTimelineClip {
    id: TimelineClipId,
    ordinal: u32,
    shot_id: ShotId,
    variant_id: VariantId,
    asset_id: MediaAssetId,
    source_start_us: i64,
    source_duration_us: i64,
}

struct MediaExport {
    id: MediaExportId,
    timeline_id: TimelineId,
    timeline_revision: u64,
    request_fingerprint: ContentHash,
    state: ExportState,
    stage: ExportStageName,
    progress_basis_points: u16,
    cancel_requested_at: Option<Timestamp>,
    error: Option<StructuredExportError>,
}

enum ExportState {
    Validating,
    Queued,
    Running,
    Verifying,
    Publishing,
    Completed,
    Failed,
    Cancelled,
    Interrupted,
}
```

Persist an export input snapshot and stage records in addition to the parent:

```text
media_export_inputs
  export_id, ordinal, timeline_clip_id, asset_id, asset_content_hash,
  source_start_us, source_duration_us

media_export_stages
  export_id, ordinal, kind, state, fingerprint, attempt_count,
  intermediate_asset_id, progress, started_at, completed_at, error_json
```

The snapshot also pins:

- the typed output profile and its schema version;
- output width/height, fit policy, pixel format, codec/profile, frame rate and
  time base;
- audio policy, codec, sample rate and channel layout;
- the FFmpeg/ffprobe binary fingerprint and supported-feature identity; and
- the timeline revision, exact clip order, asset hashes, and source ranges.

Do not store floating-point seconds as the authority. Store non-negative integer
microseconds for the UI/domain boundary, then convert to one declared rational
time base with checked arithmetic and a documented rounding rule. Compute frame
boundaries once per immutable export snapshot. For example, a 29.97 fps profile
must be represented as `30000/1001`, not `29.97`.

### Timeline invariants

1. A timeline contains exactly one ordered video track for PR16.
2. Clip IDs are stable across reorder; ordinals are unique, dense, and
   transactional.
3. Every clip belongs to the same workflow, references the selected variant of
   its shot, and resolves to an available local asset with a verified content
   hash.
4. `source_start_us >= 0`, `source_duration_us > 0`, arithmetic cannot overflow,
   and the end is within the probe-verified available duration plus one declared
   time-base tolerance.
5. Timeline edits increment `revision`. They never mutate an already-created
   export snapshot.
6. If a shot selection changes, the timeline reports the affected clip as stale
   and requires an explicit update; a running export continues from its pinned
   snapshot.
7. Empty timelines, duplicate clip IDs, missing assets, and unresolved selections
   cannot be exported.
8. PR16 supports hard cuts only. No transition is implied by adjacent clips.

### Export graph and compatibility gate

`Validate` probes all inputs and resolves an allowlisted output preset. It never
executes an untrusted URL, protocol, filter expression, codec name, or arbitrary
extra argument.

Each `NormalizeClip` stage decodes its source range and produces the same explicit
profile. This handles common mismatches instead of hoping concat will:

- resolution and sample/display aspect ratio;
- orientation/rotation and the selected contain/crop fit policy;
- pixel format, color metadata policy, frame rate, and time base;
- video codec/profile/level and encoder parameters; and
- audio-present versus audio-absent inputs, sample rate, channel layout, and an
  explicit silence/no-audio policy.

After all intermediates have been probed for exact compatibility, `Concatenate`
uses a generated ffconcat manifest and stream copy. A future optimized fast path
may bypass normalization only when probe-derived signatures match exactly and
trim boundaries are known safe. It is not required for PR16.

`Verify` uses ffprobe and filesystem checks to require:

- a non-empty regular file and successful child exit;
- the expected container and stream count;
- expected width, height, pixel format, frame rate/time base, and audio profile;
- duration within a declared frame/sample tolerance; and
- a computed content hash.

Only then may the output be registered as a managed media asset and linked to all
input assets with export lineage. `Publish` copies or moves the verified artifact
to a user destination through a unique same-directory partial file and an atomic
final rename. Render completion and destination publication are separate durable
facts, so a failed publication can be retried without rendering again.

The no-overwrite rename must use the platform primitive instead of assuming hard
links exist. On Windows, [`MoveFileExW`](https://learn.microsoft.com/en-us/windows/win32/api/winbase/nf-winbase-movefileexw)
without `MOVEFILE_REPLACE_EXISTING` preserves exclusive destination creation and
supports same-directory moves on drive and UNC targets. Linux provides atomic
[`renameat2(..., RENAME_NOREPLACE)`](https://man7.org/linux/man-pages/man2/renameat2.2.html).
Apple exposes volume support for
[`renamex_np(..., RENAME_EXCL)`](https://developer.apple.com/documentation/foundation/urlresourcekey/volumesupportsexclusiverenamingkey?language=objc).
The coordinator must fail closed if the target volume cannot provide an atomic
exclusive rename; it must never fall back to an overwrite-capable rename.

## Safe process and path contract

1. Invoke a verified FFmpeg executable directly, with the executable and
   `Vec<OsString>` arguments separate. Never invoke a shell or build one command
   string.
2. Construct argv only from typed, allowlisted presets and validated numbers.
   Asset names, prompts, and other user text never become filters or options.
3. Create a private per-export staging directory. Stage every input there under a
   generated portable name such as `segment-000000.mp4`.
4. Generate `ffconcat version 1.0` with only these relative names, set the child
   working directory to the staging directory, and retain concat `safe=1`.
   Original paths never appear in the manifest.
5. Reject symlink/junction/reparse-point escape and any input that resolves
   outside the approved managed store or export staging directory. Do not accept
   remote protocols for this local MVP.
6. A filename beginning with `-` is always a value after its option, never a
   token that can become another option. Generated staging names avoid the issue
   altogether.
7. Create output partials with exclusive-create semantics. Never delete or
   overwrite a pre-existing target without explicit, current user consent and a
   collision policy that is part of the export request.
8. Keep stdout/stderr bounded and redact private absolute paths in persisted logs
   and UI errors. Preserve a structured internal diagnostic without exposing
   prompts, credentials, or unrelated filesystem names.

On Windows, test native Unicode paths, long paths, drive-letter and UNC forms,
and reparse points. Path validation must operate on filesystem path objects, not
UTF-8 string concatenation or shell quoting rules.

## Progress, cancellation, and recovery

Run FFmpeg with a machine channel such as:

```text
-nostdin -progress pipe:1 -stats_period 0.25
```

The parser accepts fragmented reads, bounds key/value sizes, ignores unknown
keys, rejects invalid numbers, and makes visible progress monotonic. Stage
weights are derived from expected media duration/work, not invented from the
number of log lines. `progress=end` completes only the protocol stream; the
child exit result and artifact verification remain authoritative.

Cancellation is a persisted state transition:

1. transactionally set `cancel_requested_at`;
2. stop scheduling new stages;
3. ask the exact tracked child to terminate;
4. wait a bounded grace period and then force-kill that child if needed;
5. await process reaping, record `Cancelled`, and remove only owned partials;
6. never delete an existing destination or a previously verified managed asset.

Do not reattach to a stored PID after application restart because of PID reuse
and lost pipe ownership. On startup, convert orphaned `Running`, `Verifying`, or
`Publishing` stages to `Interrupted`, inspect only owned files, and retry from a
safe boundary.

Partial FFmpeg mux/transcode output is not resumable. Retry the current stage
from the beginning. Completed normalization stages may be reused only if their
fingerprint, content hash, output probe signature, and FFmpeg identity all still
match. This yields bounded recovery without pretending a corrupt partial is a
checkpoint.

Crash boundaries that need explicit transactions:

- child succeeded but the stage completion transaction did not commit;
- stage completion committed but temporary cleanup did not run;
- final render verified but managed-asset registration did not commit;
- managed render committed but user-destination publication failed; and
- atomic destination rename succeeded but the final DB update did not commit.

Each recovery action must be idempotent and must distinguish owned partials from
user files.

## Preview contract

Preview does not need a second render engine. The desktop player can walk the
ordered clip list, seek to each source start, stop at its exclusive calculated
end, and advance to the next clip. The preview uses the same immutable timeline
revision and rational boundary conversion as export.

This is an editorial preview, not proof of encoded output. Orientation, color,
scaling, frame pacing, and audio normalization may differ until the export is
rendered, so the UI must label the distinction. A low-cost generated preview is a
later optimization, not a PR16 requirement.

## Tests required for PR16

### Timeline and range tests

- append, insert, reorder, remove, and trim keep stable clip IDs and dense unique
  ordinals;
- a reorder changes preview/export order without changing source lineage;
- edits increment timeline revision and never mutate an existing export input
  snapshot;
- reject negative start, zero/negative duration, overflow, out-of-range trim,
  empty timelines, duplicate clips, missing assets, and variants not selected for
  that shot;
- table-test boundary conversion at 24, 25, 30, `30000/1001`, 50, and 60 fps,
  including sub-frame starts and last-frame rounding;
- mixed orientation/aspect ratio follows the explicit contain/crop preset; and
- mixed audio/no-audio inputs follow the declared silence/no-audio policy without
  accumulated clip drift.

### Argument and path tests

- filenames and destinations containing spaces, apostrophes, quotes, Unicode,
  leading dashes, ampersands, percent signs, semicolons, newlines, and parentheses
  remain one argv value and never become an option or shell token;
- `..`, device names, alternate data streams, UNC paths, long Windows paths, and
  symlink/junction/reparse escape are rejected or handled by the documented local
  path policy;
- the ffconcat manifest contains only generated relative portable names and runs
  with `safe=1`;
- remote URL/protocol input and arbitrary filter/extra-argument fields are
  impossible through the typed API; and
- destination collisions require explicit overwrite/versioning consent, while
  cancellation cannot delete a pre-existing target.

### Process and recovery tests

- parse fragmented, malformed, oversized, duplicated, missing, and out-of-order
  progress keys without panic or progress regression;
- `progress=end` followed by nonzero exit or invalid output fails the stage;
- cancel before spawn, during normalize, during concat, during verify, and during
  publish reaches a terminal state and reaps the child;
- a hung child is force-killed only after the grace period;
- application restart marks stale live stages interrupted and never reattaches to
  an old PID;
- completed normalized intermediates are reused only for an exact fingerprint;
- injected crashes at every transaction boundary recover idempotently; and
- concurrent exports respect an explicit resource limit and never share mutable
  staging files.

### Artifact and lineage tests

- zero-byte, truncated, wrong-stream, wrong-dimension, duration-mismatch, and
  corrupt outputs never become completed exports;
- a verified output has a content hash, probe metadata, timeline revision,
  ordered input hashes/ranges, output profile, and FFmpeg identity;
- managed render completion is preserved when publication fails, and publication
  retry does not re-render;
- atomic publication never exposes a partial final filename; and
- cleanup removes only export-owned temporary files and retains evidence needed
  for a bounded structured error.

### Integration fixture matrix

Use tiny deterministic fixtures covering:

- matching H.264/AAC clips for the concat path;
- mismatched resolution, frame rate/time base, pixel format, codec, and audio
  layout for normalization;
- inter-frame clips trimmed away from keyframes;
- one clip without audio and one with audio;
- portrait plus landscape inputs;
- a Unicode/space-heavy Windows path; and
- deterministic fake-FFmpeg processes for progress, cancel, crash, and corrupt
  output tests, plus at least one real FFmpeg end-to-end export.

## Patterns deliberately avoided

- **UI array as authority:** order and range are durable domain data.
- **Live process as job state:** progress and cancellation are persisted.
- **One opaque FFmpeg command:** stages are separately fingerprinted and
  recoverable.
- **Concat as normalization:** FFmpeg documents strict compatibility; validate or
  normalize first.
- **Shell command strings:** executable and argv remain structured path values.
- **Direct render into the user's final file:** verify a private artifact, then
  publish atomically.
- **Resume a partial mux/transcode:** restart that stage; reuse only completed,
  verified intermediates.
- **Progress as success:** require exit, probe verification, hash, and durable
  registration.
- **Copied NLE implementation:** Kdenlive/MLT are licensing and complexity
  boundaries, not libraries to vendor for this MVP.
- **Wave 5 scope creep:** no Extend/Edit/Motion Transfer or general DAG editor.

## PR16 completion checklist

- One ordered, durable timeline with selected variants and validated source
  ranges.
- Deterministic preview order and hard-cut boundaries.
- Immutable export snapshots and persisted stage/attempt state.
- Typed allowlisted output presets; no arbitrary FFmpeg arguments.
- Private safe-path staging, normalized intermediates, safe concat, output probe,
  content hash, lineage, and atomic publish.
- Monotonic machine progress, durable cancellation, bounded termination, and
  restart recovery.
- FFmpeg build/license provenance recorded and packaging obligations reviewed.
- Focused unit, fake-process, recovery, path-injection, and real FFmpeg
  integration tests passing.
- No Wave 5 editing capability and no copied GPL implementation.
