import { useEffect, useMemo, useRef, useState, type ReactNode } from 'react';
import { convertFileSrc } from '@tauri-apps/api/core';
import { save as showSaveDialog } from '@tauri-apps/plugin-dialog';
import { ArrowLeft, ArrowRight, Film, FolderOpen, Loader2, Pause, Play, Plus, RefreshCw, Scissors, Trash2, X } from 'lucide-react';
import { toast } from 'sonner';
import * as api from '../../lib/api';
import type {
  VideoTimelineClipRecord,
  VideoTimelineOutputProfile,
  VideoTimelineSnapshot,
  VideoWorkflowSnapshot,
} from '../../types/videoWorkflow';

const inputClass = 'h-8 rounded-md border border-border/70 bg-surface-0 px-2 text-xs text-text-primary outline-none focus:border-accent/70 disabled:opacity-50';

function ActionButton({ children, onClick, disabled, title, danger = false }: { children: ReactNode; onClick?: () => void; disabled?: boolean; title?: string; danger?: boolean }) {
  return <button type="button" title={title} onClick={onClick} disabled={disabled} className={`inline-flex h-8 items-center justify-center gap-1.5 rounded-md border px-2.5 text-xs font-medium transition-colors disabled:pointer-events-none disabled:opacity-45 ${danger ? 'border-danger/40 text-danger hover:bg-danger/10' : 'border-border/70 text-text-secondary hover:bg-surface-2 hover:text-text-primary'}`}>{children}</button>;
}

function seconds(microseconds: number) {
  return microseconds / 1_000_000;
}

function exportProfile(resolution: '720p' | '1080p', fps: '24' | '29.97' | '30' | '60', aspectRatio: string): VideoTimelineOutputProfile {
  const long = resolution === '1080p' ? 1920 : 1280;
  const short = resolution === '1080p' ? 1080 : 720;
  let width = long;
  let height = short;
  if (aspectRatio === '9:16' || aspectRatio === '3:4') [width, height] = [short, long];
  if (aspectRatio === '1:1') width = height = short;
  if (aspectRatio === '4:3') width = resolution === '1080p' ? 1440 : 960;
  if (aspectRatio === '3:4') height = resolution === '1080p' ? 1440 : 960;
  if (aspectRatio === '21:9') width = resolution === '1080p' ? 2560 : 1680;
  const [fpsNumerator, fpsDenominator] = fps === '29.97' ? [30_000, 1001] : [Number(fps), 1];
  return {
    schemaVersion: 1,
    width,
    height,
    fit: 'contain',
    fpsNumerator,
    fpsDenominator,
    pixelFormat: 'yuv420p',
    videoCodec: 'h264',
    videoProfile: 'high',
    videoLevel: 52,
    videoTimeBaseNumerator: 1,
    videoTimeBaseDenominator: 90000,
    colorPrimaries: 'bt709',
    colorTransfer: 'bt709',
    colorSpace: 'bt709',
    colorRange: 'tv',
    videoPreset: 'medium',
    videoCrf: 20,
    audioCodec: 'aac',
    audioSampleRate: 48000,
    audioChannelLayout: 'stereo',
  };
}

function ClipRangeEditor({ clip, timeline, busy, onChange }: { clip: VideoTimelineClipRecord; timeline: VideoTimelineSnapshot; busy: boolean; onChange: (snapshot: VideoTimelineSnapshot) => void }) {
  const [start, setStart] = useState(seconds(clip.sourceStartUs).toFixed(3));
  const [duration, setDuration] = useState(seconds(clip.sourceDurationUs).toFixed(3));
  useEffect(() => {
    setStart(seconds(clip.sourceStartUs).toFixed(3));
    setDuration(seconds(clip.sourceDurationUs).toFixed(3));
  }, [clip.revision, clip.sourceStartUs, clip.sourceDurationUs]);
  const saveRange = async () => {
    const sourceStartUs = Math.round(Number(start) * 1_000_000);
    const sourceDurationUs = Math.round(Number(duration) * 1_000_000);
    if (!Number.isSafeInteger(sourceStartUs) || !Number.isSafeInteger(sourceDurationUs) || sourceStartUs < 0 || sourceDurationUs <= 0) {
      toast.error('Start must be non-negative and duration must be positive');
      return;
    }
    try {
      onChange(await api.updateVideoTimelineClip({
        workflowId: timeline.timeline.workflowId,
        expectedTimelineRevision: timeline.timeline.revision,
        clipId: clip.id,
        expectedClipRevision: clip.revision,
        sourceStartUs,
        sourceDurationUs,
      }));
    } catch (error) {
      toast.error(String(error));
    }
  };
  return <div className="flex flex-wrap items-center gap-1.5">
    <label className="flex items-center gap-1 text-[10px] text-text-tertiary">In <input className={`${inputClass} w-20`} inputMode="decimal" value={start} onChange={(event) => setStart(event.target.value)} aria-label={`${clip.shotTitle} source start seconds`} /></label>
    <label className="flex items-center gap-1 text-[10px] text-text-tertiary">Duration <input className={`${inputClass} w-20`} inputMode="decimal" value={duration} onChange={(event) => setDuration(event.target.value)} aria-label={`${clip.shotTitle} source duration seconds`} /></label>
    <ActionButton onClick={() => void saveRange()} disabled={busy || (start === seconds(clip.sourceStartUs).toFixed(3) && duration === seconds(clip.sourceDurationUs).toFixed(3))}><Scissors className="h-3.5 w-3.5" /> Apply</ActionButton>
  </div>;
}

export function VideoTimelinePanel({ workflow, busy, onBusyChange }: { workflow: VideoWorkflowSnapshot; busy: boolean; onBusyChange: (busy: boolean) => void }) {
  const [snapshot, setSnapshot] = useState<VideoTimelineSnapshot | null>(null);
  const [paths, setPaths] = useState<Record<string, string>>({});
  const [previewIndex, setPreviewIndex] = useState(0);
  const [playing, setPlaying] = useState(false);
  const [resolution, setResolution] = useState<'720p' | '1080p'>('1080p');
  const [fps, setFps] = useState<'24' | '29.97' | '30' | '60'>('30');
  const [exportIntentKey, setExportIntentKey] = useState<string | null>(null);
  const videoRef = useRef<HTMLVideoElement>(null);
  const workflowId = workflow.workflow.id;

  const reload = async () => setSnapshot(await api.getVideoTimeline(workflowId));
  useEffect(() => {
    setSnapshot(null);
    setPreviewIndex(0);
    setPlaying(false);
    setExportIntentKey(null);
    void reload().catch((error) => toast.error(String(error)));
  }, [workflowId]);
  useEffect(() => {
    if (!snapshot || !snapshot.exports.some((item) => !['completed', 'failed', 'cancelled'].includes(item.state))) return;
    const timer = window.setInterval(() => void reload().catch(() => undefined), 1200);
    return () => window.clearInterval(timer);
  }, [snapshot?.timeline.id, snapshot?.exports.map((item) => `${item.id}:${item.revision}`).join('|')]);
  useEffect(() => {
    if (!snapshot) return;
    let active = true;
    Promise.all(snapshot.clips.map(async (clip) => {
      try { return [clip.id, await api.resolveMediaGenerationAssetPath(clip.assetId)] as const; } catch { return null; }
    })).then((entries) => {
      if (active) setPaths(Object.fromEntries(entries.filter((entry): entry is readonly [string, string] => entry != null)));
    });
    return () => { active = false; };
  }, [snapshot?.clips.map((clip) => `${clip.id}:${clip.assetId}`).join('|')]);

  const run = async (action: () => Promise<void>) => {
    onBusyChange(true);
    try { await action(); } catch (error) { toast.error(String(error)); } finally { onBusyChange(false); }
  };
  const selectedNotAdded = useMemo(() => workflow.shots.filter((shot) => shot.shot.selectedVariantId && !snapshot?.clips.some((clip) => clip.shotId === shot.shot.id)), [snapshot?.clips, workflow.shots]);
  const currentClip = snapshot?.clips[previewIndex] ?? null;
  const currentPath = currentClip ? paths[currentClip.id] : null;

  const addClip = (shotId: string, variantId: string) => snapshot && void run(async () => {
    setSnapshot(await api.addVideoTimelineClip({ workflowId, expectedTimelineRevision: snapshot.timeline.revision, shotId, variantId }));
  });
  const moveClip = (clipId: string, direction: -1 | 1) => snapshot && void run(async () => {
    const ids = snapshot.clips.map((clip) => clip.id);
    const index = ids.indexOf(clipId);
    const target = index + direction;
    if (index < 0 || target < 0 || target >= ids.length) return;
    [ids[index], ids[target]] = [ids[target], ids[index]];
    setSnapshot(await api.reorderVideoTimelineClips({ workflowId, expectedTimelineRevision: snapshot.timeline.revision, orderedClipIds: ids }));
    setPreviewIndex(target);
  });
  const refreshClip = (clip: VideoTimelineClipRecord) => snapshot && void run(async () => {
    setSnapshot(await api.refreshVideoTimelineClip({ workflowId, expectedTimelineRevision: snapshot.timeline.revision, clipId: clip.id, expectedClipRevision: clip.revision }));
  });
  const removeClip = (clip: VideoTimelineClipRecord) => snapshot && void run(async () => {
    setSnapshot(await api.removeVideoTimelineClip({ workflowId, expectedTimelineRevision: snapshot.timeline.revision, clipId: clip.id, expectedClipRevision: clip.revision }));
    setPreviewIndex((value) => Math.max(0, Math.min(value, snapshot.clips.length - 2)));
  });
  const togglePreview = async () => {
    const video = videoRef.current;
    if (!video || !currentClip) return;
    if (playing) { video.pause(); setPlaying(false); return; }
    if (video.currentTime < seconds(currentClip.sourceStartUs) || video.currentTime >= seconds(currentClip.sourceStartUs + currentClip.sourceDurationUs)) video.currentTime = seconds(currentClip.sourceStartUs);
    await video.play();
    setPlaying(true);
  };
  const advancePreview = () => {
    if (!snapshot || previewIndex >= snapshot.clips.length - 1) { setPlaying(false); return; }
    setPreviewIndex((value) => value + 1);
  };
  useEffect(() => {
    if (!currentClip || !videoRef.current) return;
    videoRef.current.currentTime = seconds(currentClip.sourceStartUs);
    if (playing) void videoRef.current.play();
  }, [currentClip?.id]);

  const startExport = snapshot && (() => void run(async () => {
    if (snapshot.clips.length === 0 || snapshot.clips.some((clip) => clip.stale)) throw new Error('Add clips and refresh stale selections before export');
    const destination = await showSaveDialog({ defaultPath: `${workflow.workflow.title || 'nexa-video'}.mp4`, filters: [{ name: 'MPEG-4 video', extensions: ['mp4'] }] });
    if (!destination) return;
    const idempotencyKey = exportIntentKey ?? crypto.randomUUID();
    setExportIntentKey(idempotencyKey);
    await api.createVideoTimelineExport({
      workflowId,
      expectedTimelineRevision: snapshot.timeline.revision,
      idempotencyKey,
      destinationPath: destination,
      outputProfile: exportProfile(resolution, fps, workflow.workflow.aspectRatio),
    });
    setExportIntentKey(null);
    await reload();
    toast.success('Durable FFmpeg export started');
  }));
  const retryExport = (exportId: string, revision: number, defaultPath: string) => void run(async () => {
    const destination = await showSaveDialog({ defaultPath, filters: [{ name: 'MPEG-4 video', extensions: ['mp4'] }] });
    if (!destination) return;
    await api.retryVideoTimelineExport({ exportId, expectedRevision: revision, destinationPath: destination });
    await reload();
    toast.success('Export retry started from the last verified boundary');
  });

  if (!snapshot) return <section className="rounded-xl border border-border/70 bg-surface-1 p-5 text-sm text-text-tertiary"><Loader2 className="mr-2 inline h-4 w-4 animate-spin" />Loading durable timeline</section>;
  return <section className="rounded-xl border border-border/70 bg-surface-1">
    <header className="flex flex-wrap items-center justify-between gap-3 border-b border-border/60 px-4 py-3">
      <div><div className="flex items-center gap-2"><Film className="h-4 w-4 text-accent" /><h2 className="text-sm font-semibold">Timeline & Export</h2></div><p className="mt-1 text-[11px] text-text-tertiary">Single video track · hard cuts · immutable export snapshots · editorial preview</p></div>
      <div className="flex flex-wrap items-center gap-2"><select className={inputClass} value={resolution} onChange={(event) => setResolution(event.target.value as '720p' | '1080p')} aria-label="Export resolution"><option value="720p">720p</option><option value="1080p">1080p</option></select><select className={inputClass} value={fps} onChange={(event) => setFps(event.target.value as typeof fps)} aria-label="Export frame rate"><option value="24">24 fps</option><option value="29.97">29.97 fps</option><option value="30">30 fps</option><option value="60">60 fps</option></select><ActionButton onClick={startExport ?? undefined} disabled={busy || snapshot.clips.length === 0 || snapshot.clips.some((clip) => clip.stale)}><Film className="h-3.5 w-3.5" /> Export MP4</ActionButton></div>
    </header>
    <div className="grid gap-4 p-4 xl:grid-cols-[minmax(0,1.25fr)_minmax(320px,0.75fr)]">
      <div className="space-y-3">
        {selectedNotAdded.length > 0 && <div className="rounded-lg border border-accent/25 bg-accent/5 p-3"><div className="mb-2 text-[11px] font-medium text-text-secondary">Selected shots ready for the timeline</div><div className="flex flex-wrap gap-2">{selectedNotAdded.map((shot) => <ActionButton key={shot.shot.id} onClick={() => addClip(shot.shot.id, shot.shot.selectedVariantId!)} disabled={busy}><Plus className="h-3.5 w-3.5" /> {shot.shot.title}</ActionButton>)}</div></div>}
        {snapshot.clips.length === 0 ? <div className="rounded-lg border border-dashed border-border p-8 text-center text-xs text-text-tertiary">Select a completed variant for a shot, then add it to this durable timeline.</div> : <div className="space-y-2">{snapshot.clips.map((clip, index) => <article key={clip.id} className={`rounded-lg border p-3 ${index === previewIndex ? 'border-accent/45 bg-accent/5' : 'border-border/60 bg-surface-0'}`}>
          <div className="flex flex-wrap items-start gap-2"><button type="button" onClick={() => setPreviewIndex(index)} className="min-w-0 flex-1 text-left"><span className="text-[10px] font-semibold text-text-tertiary">{String(index + 1).padStart(2, '0')}</span><span className="ml-2 text-xs font-medium">{clip.shotTitle}</span><span className="ml-2 text-[10px] text-text-tertiary">{seconds(clip.sourceDurationUs).toFixed(2)}s · {clip.assetContentHash.slice(0, 10)}…</span></button>{clip.stale && <span className="rounded-full border border-warning/40 bg-warning/5 px-2 py-0.5 text-[10px] font-semibold text-warning">selection changed</span>}<ActionButton title="Move clip earlier" onClick={() => moveClip(clip.id, -1)} disabled={busy || index === 0}><ArrowLeft className="h-3.5 w-3.5" /></ActionButton><ActionButton title="Move clip later" onClick={() => moveClip(clip.id, 1)} disabled={busy || index === snapshot.clips.length - 1}><ArrowRight className="h-3.5 w-3.5" /></ActionButton><ActionButton title="Remove clip" onClick={() => removeClip(clip)} disabled={busy} danger><Trash2 className="h-3.5 w-3.5" /></ActionButton></div>
          <div className="mt-2 flex flex-wrap items-center justify-between gap-2"><ClipRangeEditor clip={clip} timeline={snapshot} busy={busy} onChange={setSnapshot} />{clip.stale && <ActionButton onClick={() => refreshClip(clip)} disabled={busy}><RefreshCw className="h-3.5 w-3.5" /> Use current selection</ActionButton>}</div>
        </article>)}</div>}
      </div>
      <div className="space-y-3">
        <div className="overflow-hidden rounded-lg border border-border/60 bg-black">{currentClip && currentPath ? <video key={currentClip.id} ref={videoRef} src={convertFileSrc(currentPath)} playsInline className="aspect-video w-full" onLoadedMetadata={(event) => { event.currentTarget.currentTime = seconds(currentClip.sourceStartUs); }} onTimeUpdate={(event) => { if (event.currentTarget.currentTime >= seconds(currentClip.sourceStartUs + currentClip.sourceDurationUs)) { event.currentTarget.pause(); advancePreview(); } }} onEnded={advancePreview} /> : <div className="flex aspect-video items-center justify-center text-xs text-white/60">Add a local verified clip to preview</div>}<div className="flex items-center justify-between border-t border-white/10 bg-black/80 p-2"><ActionButton onClick={() => void togglePreview()} disabled={!currentClip || !currentPath}>{playing ? <Pause className="h-3.5 w-3.5" /> : <Play className="h-3.5 w-3.5" />}{playing ? 'Pause' : 'Preview cuts'}</ActionButton><span className="text-[10px] text-white/55">{currentClip ? `${previewIndex + 1}/${snapshot.clips.length} · ${seconds(currentClip.sourceStartUs).toFixed(2)}s in` : 'No clip'}</span></div></div>
        <div className="space-y-2"><div className="text-[11px] font-semibold text-text-secondary">Export history</div>{snapshot.exports.length === 0 && <p className="rounded-lg border border-dashed border-border p-5 text-center text-xs text-text-tertiary">Exports run Normalize → Concat → Verify → Publish and appear here.</p>}{snapshot.exports.map((item) => <article key={item.id} className="rounded-lg border border-border/60 bg-surface-0 p-3"><div className="flex items-start gap-2"><span className="rounded-full border border-border/70 px-2 py-0.5 text-[10px] font-semibold uppercase">{item.cancellationRequestedAt && !['completed', 'failed', 'cancelled'].includes(item.state) ? 'cancelling' : item.state}</span><div className="min-w-0 flex-1"><div className="truncate text-xs font-medium">{item.destinationPath.split(/[\\/]/).pop()}</div><div className="mt-0.5 text-[10px] text-text-tertiary">{item.currentStage} · {(item.progressBasisPoints / 100).toFixed(1)}% · timeline r{item.timelineRevision}</div></div></div><div className="mt-2 h-1.5 overflow-hidden rounded-full bg-surface-3"><div className="h-full bg-accent transition-[width]" style={{ width: `${item.progressBasisPoints / 100}%` }} /></div>{item.error && <p className="mt-2 text-[11px] text-warning">{String(item.error.message ?? item.error.code ?? 'Export failed')}</p>}<div className="mt-2 flex justify-end gap-2">{item.state === 'failed' && <ActionButton onClick={() => retryExport(item.id, item.revision, item.destinationPath)} disabled={busy}><RefreshCw className="h-3.5 w-3.5" /> Retry</ActionButton>}{item.state === 'completed' && item.outputAssetId && <ActionButton onClick={() => void api.resolveMediaGenerationAssetPath(item.outputAssetId!).then(api.showInFileExplorer).catch((error) => toast.error(String(error)))}><FolderOpen className="h-3.5 w-3.5" /> Show managed copy</ActionButton>}{!['completed', 'failed', 'cancelled'].includes(item.state) && !item.cancellationRequestedAt && <ActionButton danger onClick={() => void run(async () => { await api.cancelVideoTimelineExport({ exportId: item.id, expectedRevision: item.revision }); await reload(); })}><X className="h-3.5 w-3.5" /> Cancel</ActionButton>}</div></article>)}</div>
      </div>
    </div>
  </section>;
}
