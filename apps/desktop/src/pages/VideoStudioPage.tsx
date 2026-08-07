import { useCallback, useEffect, useMemo, useRef, useState, type ReactNode } from 'react';
import { convertFileSrc } from '@tauri-apps/api/core';
import { ArrowDown, ArrowUp, Check, Clapperboard, ImagePlus, KeyRound, Loader2, Pause, Play, Plus, RefreshCw, RotateCcw, Save, ShieldCheck, Trash2, X } from 'lucide-react';
import { toast } from 'sonner';
import * as api from '../lib/api';
import { defaultDuration, formatMicros, mayCancelVariant, mayRetryVariant, operationCapability, queueBucket, selectableVideoModels, variantStatusLabel } from '../lib/videoWorkflowViewModel';
import type { MediaOperation } from '../types/mediaGeneration';
import type { VideoInputRole, VideoProviderConnectionRecord, VideoProviderPreset, VideoQueueDisclosure, VideoShotInput, VideoWorkflowShotSnapshot, VideoWorkflowSnapshot, VideoWorkflowVariantRecord } from '../types/videoWorkflow';

const inputClass = 'h-9 w-full rounded-md border border-border/70 bg-surface-0 px-3 text-sm text-text-primary outline-none transition-colors focus:border-accent/70 disabled:cursor-not-allowed disabled:opacity-55';
const areaClass = 'w-full rounded-md border border-border/70 bg-surface-0 px-3 py-2 text-sm text-text-primary outline-none transition-colors focus:border-accent/70 disabled:cursor-not-allowed disabled:opacity-55';

function Panel({ title, actions, children }: { title: string; actions?: ReactNode; children: ReactNode }) {
  return <section className="rounded-xl border border-border/70 bg-surface-1"><header className="flex min-h-11 items-center justify-between gap-3 border-b border-border/60 px-4 py-2.5"><h2 className="text-sm font-semibold">{title}</h2>{actions}</header>{children}</section>;
}

function Button({ children, onClick, disabled, tone = 'secondary', title }: { children: ReactNode; onClick?: () => void; disabled?: boolean; tone?: 'primary' | 'secondary' | 'danger'; title?: string }) {
  const palette = tone === 'primary' ? 'border-accent bg-accent text-white hover:bg-accent-hover' : tone === 'danger' ? 'border-danger/40 text-danger hover:bg-danger/10' : 'border-border/70 text-text-secondary hover:bg-surface-2 hover:text-text-primary';
  return <button type="button" title={title} onClick={onClick} disabled={disabled} className={`inline-flex h-8 items-center justify-center gap-1.5 rounded-md border px-2.5 text-xs font-medium transition-colors disabled:pointer-events-none disabled:opacity-45 ${palette}`}>{children}</button>;
}

function operationLabel(value: MediaOperation) {
  return value.split('_').map((part) => part[0].toUpperCase() + part.slice(1)).join(' ');
}

function variantTone(variant: VideoWorkflowVariantRecord) {
  const bucket = queueBucket(variant);
  if (bucket === 'complete') return 'border-success/35 bg-success/5 text-success';
  if (bucket === 'attention') return 'border-warning/35 bg-warning/5 text-warning';
  if (bucket === 'active') return 'border-accent/35 bg-accent/5 text-accent-hover';
  return 'border-border/70 bg-surface-2 text-text-tertiary';
}

function SynchronizedCompare({ variants, selectedId, busy, onSelect }: { variants: VideoWorkflowVariantRecord[]; selectedId: string | null; busy: boolean; onSelect: (variant: VideoWorkflowVariantRecord) => void }) {
  const [leftId, setLeftId] = useState('');
  const [rightId, setRightId] = useState('');
  const [paths, setPaths] = useState<Record<string, string>>({});
  const [playing, setPlaying] = useState(false);
  const [time, setTime] = useState(0);
  const [duration, setDuration] = useState(0);
  const leftRef = useRef<HTMLVideoElement>(null);
  const rightRef = useRef<HTMLVideoElement>(null);
  const variantFingerprint = variants.map((variant) => `${variant.id}:${variant.job.outputAssetId ?? ''}`).join('|');
  useEffect(() => {
    let active = true;
    setLeftId((value) => variants.some((variant) => variant.id === value) ? value : variants[0]?.id ?? '');
    setRightId((value) => variants.some((variant) => variant.id === value) && value !== variants[0]?.id ? value : variants[1]?.id ?? variants[0]?.id ?? '');
    Promise.all(variants.map(async (variant) => {
      if (!variant.job.outputAssetId) return null;
      try { return [variant.id, await api.resolveMediaGenerationAssetPath(variant.job.outputAssetId)] as const; } catch { return null; }
    })).then((entries) => { if (active) setPaths(Object.fromEntries(entries.filter((entry): entry is readonly [string, string] => entry != null))); });
    return () => { active = false; };
  }, [variantFingerprint]);
  const left = variants.find((variant) => variant.id === leftId) ?? null;
  const right = variants.find((variant) => variant.id === rightId) ?? null;
  const seek = (next: number) => {
    setTime(next);
    if (leftRef.current) leftRef.current.currentTime = next;
    if (rightRef.current) rightRef.current.currentTime = next;
  };
  const toggle = async () => {
    if (playing) {
      leftRef.current?.pause(); rightRef.current?.pause(); setPlaying(false); return;
    }
    if (leftRef.current) await leftRef.current.play();
    if (rightRef.current) { rightRef.current.muted = true; await rightRef.current.play(); }
    setPlaying(true);
  };
  if (variants.length < 2) return <p className="py-8 text-center text-xs text-text-tertiary">At least two completed, content-verified variants are required for synchronized comparison.</p>;
  return <div className="space-y-3">
    <div className="grid gap-3 sm:grid-cols-2 xl:grid-cols-1 2xl:grid-cols-2">
      {[{ side: 'A', variant: left, id: leftId, otherId: rightId, setId: setLeftId, ref: leftRef, muted: false }, { side: 'B', variant: right, id: rightId, otherId: leftId, setId: setRightId, ref: rightRef, muted: true }].map((item) => <article key={item.side} className={`rounded-xl border p-2 ${item.variant?.id === selectedId ? 'border-success/55 bg-success/5' : 'border-border/70 bg-surface-0'}`}><select className={`${inputClass} mb-2`} value={item.id} onChange={(event) => item.setId(event.target.value)}>{variants.filter((variant) => variant.id !== item.otherId || variant.id === item.id).map((variant) => <option key={variant.id} value={variant.id}>{item.side} · {variant.label}</option>)}</select>{item.variant && paths[item.variant.id] ? <video ref={item.ref} src={convertFileSrc(paths[item.variant.id])} muted={item.muted} playsInline className="aspect-video w-full rounded-lg bg-black" onLoadedMetadata={(event) => { if (item.side === 'A') setDuration(event.currentTarget.duration || 0); }} onTimeUpdate={(event) => { if (item.side !== 'A') return; const next = event.currentTarget.currentTime; setTime(next); if (rightRef.current && Math.abs(rightRef.current.currentTime - next) > 0.12) rightRef.current.currentTime = next; }} onEnded={() => setPlaying(false)} /> : <div className="flex aspect-video items-center justify-center rounded-lg bg-surface-0 text-xs text-text-tertiary">Verified local output unavailable</div>}<div className="mt-2 flex justify-end"><Button onClick={() => item.variant && onSelect(item.variant)} disabled={busy || !item.variant || item.variant.id === selectedId} tone={item.variant?.id === selectedId ? 'secondary' : 'primary'}>{item.variant?.id === selectedId ? <><Check className="h-3.5 w-3.5" /> Selected</> : 'Select'}</Button></div></article>)}
    </div>
    <div className="flex items-center gap-2 rounded-lg border border-border/60 bg-surface-0 p-2"><Button onClick={() => void toggle()} disabled={!left || !right}>{playing ? <Pause className="h-3.5 w-3.5" /> : <Play className="h-3.5 w-3.5" />}{playing ? 'Pause both' : 'Play both'}</Button><input type="range" min={0} max={duration || 0} step={0.05} value={Math.min(time, duration || 0)} onChange={(event) => seek(Number(event.target.value))} className="min-w-0 flex-1" aria-label="Shared compare timeline" /><span className="text-[10px] text-text-tertiary">A audio · B muted</span></div>
  </div>;
}

function ShotCard({ item, workflowRevision, models, busy, focused, onFocus, onChange, onQueue, onMove, onDelete }: {
  item: VideoWorkflowShotSnapshot;
  workflowRevision: number;
  models: ReturnType<typeof selectableVideoModels>;
  busy: boolean;
  focused: boolean;
  onFocus: () => void;
  onChange: (snapshot: VideoWorkflowSnapshot) => void;
  onQueue: (count: number) => Promise<void>;
  onMove: (direction: -1 | 1) => void;
  onDelete: () => void;
}) {
  const hasVariants = item.variants.length > 0;
  const [draft, setDraft] = useState<VideoShotInput>(() => ({ ...item.shot }));
  const [variantCount, setVariantCount] = useState(2);
  const [referenceUrl, setReferenceUrl] = useState('');
  const [referenceRole, setReferenceRole] = useState<VideoInputRole>('reference_image');
  const [inspectingReference, setInspectingReference] = useState(false);
  useEffect(() => setDraft({ ...item.shot }), [item.shot]);
  const selected = models.find(({ model, connection }) => connection.id === draft.connectionId && model.modelId === draft.modelId && model.apiVersion === (draft.apiVersion ?? null));
  const operations = selected?.model.operationCapabilities ?? [];
  const capability = selected ? operationCapability(selected.model, draft.operation) : null;
  const durations = capability ? [...new Set(capability.durationOptions.flatMap((option) => option.durationsSeconds.length ? option.durationsSeconds : [option.minDurationSeconds ?? option.maxDurationSeconds ?? draft.durationSeconds]))] : [draft.durationSeconds];
  const resolutions = capability ? [...new Set(capability.durationOptions.map((option) => option.resolution))] : [draft.resolution];
  const imageRoles = (capability?.inputRoles ?? []).filter((role): role is VideoInputRole => role === 'first_frame' || role === 'last_frame' || role === 'reference_image');
  const activeReferenceRole = imageRoles.includes(referenceRole) ? referenceRole : imageRoles[0] ?? 'reference_image';
  const unsupportedReferenceRoles = (capability?.inputRoles ?? []).filter((role) => role === 'input_video' || role === 'reference_video' || role === 'reference_audio');

  const chooseModel = (key: string) => {
    const next = models.find(({ model, connection }) => `${connection.id}:${model.modelId}:${model.apiVersion ?? ''}` === key);
    if (!next) return;
    const nextCapability = next.model.operationCapabilities[0];
    const defaults = defaultDuration(nextCapability);
    setDraft((current) => ({ ...current, connectionId: next.connection.id, providerId: next.model.providerId, modelId: next.model.modelId, apiVersion: next.model.apiVersion, operation: nextCapability.operation, durationSeconds: defaults.durationSeconds, resolution: defaults.resolution, aspectRatio: defaults.aspectRatio, inputAssets: [], seed: nextCapability.supportsSeed ? current.seed : null, generateAudio: nextCapability.supportsAudio ? (current.generateAudio ?? true) : null, allowCrossProviderFallback: false }));
  };
  const chooseOperation = (operation: MediaOperation) => {
    if (!selected) return;
    const next = operationCapability(selected.model, operation);
    if (!next) return;
    const defaults = defaultDuration(next);
    setDraft((current) => ({ ...current, operation, durationSeconds: defaults.durationSeconds, resolution: defaults.resolution, aspectRatio: defaults.aspectRatio, inputAssets: [], seed: next.supportsSeed ? current.seed : null, generateAudio: next.supportsAudio ? (current.generateAudio ?? true) : null }));
  };
  const save = async () => {
    try {
      onChange(await api.updateVideoWorkflowShot({ workflowId: item.shot.workflowId, expectedWorkflowRevision: workflowRevision, shotId: item.shot.id, expectedShotRevision: item.shot.revision, shot: draft }));
      toast.success('Shot saved');
    } catch (error) { toast.error(String(error)); }
  };
  const inspectReference = async () => {
    if (!referenceUrl.trim() || imageRoles.length === 0) return;
    setInspectingReference(true);
    try {
      const verified = await api.inspectVideoReferenceImage(referenceUrl.trim());
      setDraft((current) => ({
        ...current,
        inputAssets: [...current.inputAssets, {
          role: activeReferenceRole,
          uri: verified.uri,
          mediaType: verified.mediaType,
          metadataVerified: true,
          byteLength: verified.byteLength,
          contentHashSha256: verified.contentHashSha256,
          localAssetId: null,
          width: verified.width,
          height: verified.height,
          durationMs: null,
          frameRate: null,
          videoCodec: null,
        }],
      }));
      setReferenceUrl('');
      toast.success('Reference image verified. Save this shot revision before queueing.');
    } catch (error) {
      toast.error(String(error));
    } finally {
      setInspectingReference(false);
    }
  };
  const moveReference = (index: number, direction: -1 | 1) => {
    const target = index + direction;
    if (target < 0 || target >= draft.inputAssets.length) return;
    setDraft((current) => {
      const inputAssets = [...current.inputAssets];
      [inputAssets[index], inputAssets[target]] = [inputAssets[target], inputAssets[index]];
      return { ...current, inputAssets };
    });
  };

  return (
    <article onClick={onFocus} className={`rounded-xl border p-4 transition-colors ${focused ? 'border-accent/55 bg-accent/5' : 'border-border/70 bg-surface-1'}`}>
      <div className="mb-3 flex items-center gap-2">
        <span className="flex h-7 w-7 items-center justify-center rounded-full bg-surface-3 text-xs font-semibold">{item.shot.ordinal + 1}</span>
        <input value={draft.title} onChange={(event) => setDraft((current) => ({ ...current, title: event.target.value }))} className="min-w-0 flex-1 bg-transparent text-sm font-semibold outline-none" aria-label="Shot title" />
        <Button onClick={() => onMove(-1)} disabled={busy} title="Move up"><ArrowUp className="h-3.5 w-3.5" /></Button>
        <Button onClick={() => onMove(1)} disabled={busy} title="Move down"><ArrowDown className="h-3.5 w-3.5" /></Button>
        <Button onClick={onDelete} disabled={busy || hasVariants} tone="danger" title={hasVariants ? 'Generated shots retain their historical snapshots' : 'Delete shot'}><Trash2 className="h-3.5 w-3.5" /></Button>
      </div>
      <textarea value={draft.prompt} onChange={(event) => setDraft((current) => ({ ...current, prompt: event.target.value }))} className={`${areaClass} min-h-20 resize-y`} placeholder="Describe this shot" />
      <div className="mt-3 grid gap-2 md:grid-cols-2 xl:grid-cols-3">
        <select value={selected ? `${selected.connection.id}:${selected.model.modelId}:${selected.model.apiVersion ?? ''}` : ''} onChange={(event) => chooseModel(event.target.value)} className={inputClass} aria-label="Provider model"><option value="">Choose a connected model</option>{models.map(({ preset, model, connection }) => <option key={`${connection.id}:${model.modelId}:${model.apiVersion ?? ''}`} value={`${connection.id}:${model.modelId}:${model.apiVersion ?? ''}`}>{preset.name} · {model.displayName} · {connection.displayName}</option>)}</select>
        <select value={draft.operation} disabled={!selected} onChange={(event) => chooseOperation(event.target.value as MediaOperation)} className={inputClass} aria-label="Operation">{operations.map((option) => <option key={option.operation} value={option.operation}>{operationLabel(option.operation)}</option>)}</select>
        <select value={draft.durationSeconds} disabled={!capability} onChange={(event) => setDraft((current) => ({ ...current, durationSeconds: Number(event.target.value) }))} className={inputClass} aria-label="Duration">{durations.map((value) => <option key={value} value={value}>{value}s</option>)}</select>
        <select value={draft.resolution} disabled={!capability} onChange={(event) => setDraft((current) => ({ ...current, resolution: event.target.value }))} className={inputClass} aria-label="Resolution">{resolutions.map((value) => <option key={value} value={value}>{value}</option>)}</select>
        <select value={draft.aspectRatio} disabled={!capability} onChange={(event) => setDraft((current) => ({ ...current, aspectRatio: event.target.value }))} className={inputClass} aria-label="Aspect ratio">{(capability?.aspectRatios ?? [draft.aspectRatio]).map((value) => <option key={value} value={value}>{value}</option>)}</select>
        {capability?.supportsAudio ? <label className="flex h-9 items-center gap-2 rounded-md border border-border/70 px-3 text-xs"><input type="checkbox" checked={draft.generateAudio ?? false} onChange={(event) => setDraft((current) => ({ ...current, generateAudio: event.target.checked }))} /> Generate audio</label> : <div />}
      </div>
      {imageRoles.length > 0 && <div className="mt-3 space-y-2 rounded-lg border border-border/60 bg-surface-0 p-3">
        <div className="flex items-center gap-2 text-xs font-medium"><ImagePlus className="h-3.5 w-3.5 text-accent" /> Verified public image references</div>
        <div className="grid gap-2 md:grid-cols-[150px_minmax(0,1fr)_auto]">
          <select className={inputClass} value={activeReferenceRole} onChange={(event) => setReferenceRole(event.target.value as VideoInputRole)} aria-label="Reference image role">{imageRoles.map((role) => <option key={role} value={role}>{role.replace(/_/g, ' ')}</option>)}</select>
          <input className={inputClass} type="url" value={referenceUrl} onChange={(event) => setReferenceUrl(event.target.value)} placeholder="https://public.example/reference.png" aria-label="Public reference image URL" />
          <Button onClick={() => void inspectReference()} disabled={busy || inspectingReference || !referenceUrl.trim()}>{inspectingReference ? <Loader2 className="h-3.5 w-3.5 animate-spin" /> : <ShieldCheck className="h-3.5 w-3.5" />} Inspect</Button>
        </div>
        <p className="text-[10px] text-text-tertiary">Nexa downloads at most 20 MiB through a public-IP-pinned connection, verifies image bytes and dimensions, then rechecks type and length before provider transfer.</p>
        {draft.inputAssets.length > 0 && <div className="space-y-1">{draft.inputAssets.map((input, index) => <div key={`${input.role}:${input.uri}:${index}`} className="flex items-center gap-2 rounded-md border border-border/50 px-2 py-1.5 text-[11px]"><span className="rounded bg-surface-3 px-1.5 py-0.5">{index + 1}</span><span className="font-medium">{input.role.replace(/_/g, ' ')}</span><span className="min-w-0 flex-1 truncate text-text-tertiary" title={input.uri}>{input.mediaType} · {input.width ?? '?'}×{input.height ?? '?'} · {input.byteLength ?? '?'} bytes</span><Button onClick={() => moveReference(index, -1)} disabled={busy || index === 0} title="Move reference earlier"><ArrowUp className="h-3 w-3" /></Button><Button onClick={() => moveReference(index, 1)} disabled={busy || index === draft.inputAssets.length - 1} title="Move reference later"><ArrowDown className="h-3 w-3" /></Button><Button onClick={() => setDraft((current) => ({ ...current, inputAssets: current.inputAssets.filter((_, candidate) => candidate !== index) }))} disabled={busy} tone="danger" title="Remove reference"><X className="h-3 w-3" /></Button></div>)}</div>}
      </div>}
      {unsupportedReferenceRoles.length > 0 && <div className="mt-3 rounded-lg border border-warning/25 bg-warning/5 px-3 py-2 text-xs text-text-secondary"><strong className="text-text-primary">Video and audio references remain fail-closed.</strong> This model also supports {unsupportedReferenceRoles.join(', ')}, but Nexa will not send those inputs until its local import and provider-upload bridge preserves verified bytes and lineage.</div>}
      <div className="mt-3 flex flex-wrap items-center justify-between gap-2 border-t border-border/50 pt-3">
        <div className="flex items-center gap-2 text-xs text-text-tertiary"><ShieldCheck className="h-3.5 w-3.5" /><span>{item.shot.providerId ?? 'No provider'} · region {item.shot.dataRegion ?? 'provider default'} · fallback off</span></div>
        <div className="flex items-center gap-2"><Button onClick={() => void save()} disabled={busy || !selected}><Save className="h-3.5 w-3.5" /> Save revision</Button><select value={variantCount} onChange={(event) => setVariantCount(Number(event.target.value))} className="h-8 rounded-md border border-border/70 bg-surface-0 px-2 text-xs">{[1, 2, 3, 4].map((count) => <option key={count} value={count}>{count} variant{count > 1 ? 's' : ''}</option>)}</select><Button tone="primary" disabled={busy || !selected || !draft.prompt.trim()} onClick={() => void onQueue(variantCount)}><Play className="h-3.5 w-3.5" /> Review & queue</Button></div>
      </div>
    </article>
  );
}

export function VideoStudioPage() {
  const [presets, setPresets] = useState<VideoProviderPreset[]>([]);
  const [connections, setConnections] = useState<VideoProviderConnectionRecord[]>([]);
  const [workflows, setWorkflows] = useState<VideoWorkflowSnapshot[]>([]);
  const [currentId, setCurrentId] = useState<string | null>(null);
  const [focusedShotId, setFocusedShotId] = useState<string | null>(null);
  const [loading, setLoading] = useState(true);
  const [busy, setBusy] = useState(false);
  const [workflowTitle, setWorkflowTitle] = useState('Untitled video');
  const [brief, setBrief] = useState('');
  const [connectionProvider, setConnectionProvider] = useState('');
  const [connectionName, setConnectionName] = useState('');
  const [apiKey, setApiKey] = useState('');
  const [pendingQueue, setPendingQueue] = useState<{ item: VideoWorkflowShotSnapshot; disclosure: VideoQueueDisclosure; idempotencyKey: string } | null>(null);

  const current = workflows.find((workflow) => workflow.workflow.id === currentId) ?? workflows[0] ?? null;
  const models = useMemo(() => selectableVideoModels(presets, connections), [presets, connections]);
  const focusedShot = current?.shots.find((shot) => shot.shot.id === focusedShotId) ?? current?.shots[0] ?? null;
  const focusedModel = focusedShot ? models.find(({ model, connection }) => connection.id === focusedShot.shot.connectionId && model.modelId === focusedShot.shot.modelId)?.model ?? null : null;
  const activeJobs = current?.queue.active ?? 0;
  const replaceWorkflow = useCallback((snapshot: VideoWorkflowSnapshot) => {
    setWorkflows((items) => items.some((item) => item.workflow.id === snapshot.workflow.id) ? items.map((item) => item.workflow.id === snapshot.workflow.id ? snapshot : item) : [snapshot, ...items]);
    setCurrentId(snapshot.workflow.id);
    setFocusedShotId((value) => value && snapshot.shots.some((shot) => shot.shot.id === value) ? value : snapshot.shots[0]?.shot.id ?? null);
  }, []);
  const reload = useCallback(async () => {
    const [nextPresets, nextConnections, nextWorkflows] = await Promise.all([api.listVideoGenerationCapabilities(), api.listVideoProviderConnections(), api.listVideoWorkflows()]);
    setPresets(nextPresets); setConnections(nextConnections); setWorkflows(nextWorkflows);
    setCurrentId((value) => value && nextWorkflows.some((item) => item.workflow.id === value) ? value : nextWorkflows[0]?.workflow.id ?? null);
    setConnectionProvider((value) => value || nextPresets.find((preset) => preset.models.some((model) => model.selectable && model.releaseStatus === 'ga'))?.providerId || '');
  }, []);
  useEffect(() => { reload().catch((error) => toast.error(String(error))).finally(() => setLoading(false)); }, [reload]);
  useEffect(() => {
    if (!currentId || activeJobs === 0) return;
    const timer = window.setInterval(() => { api.getVideoWorkflow(currentId).then(replaceWorkflow).catch(() => undefined); }, 4000);
    return () => window.clearInterval(timer);
  }, [activeJobs, currentId, replaceWorkflow]);
  useEffect(() => {
    if (!current) return;
    setWorkflowTitle(current.workflow.title);
    setBrief(typeof current.workflow.brief.summary === 'string' ? current.workflow.brief.summary : '');
  }, [current?.workflow.id, current?.workflow.revision]);
  const run = async (action: () => Promise<void>) => { setBusy(true); try { await action(); } catch (error) { toast.error(String(error)); } finally { setBusy(false); } };

  const createWorkflow = () => void run(async () => replaceWorkflow(await api.createVideoWorkflow({ title: 'Untitled video', brief: { summary: '' }, aspectRatio: '16:9', targetDurationMs: 30_000 })));
  const saveWorkflow = () => current && void run(async () => {
    replaceWorkflow(await api.updateVideoWorkflow({ workflowId: current.workflow.id, expectedRevision: current.workflow.revision, projectId: current.workflow.projectId, title: workflowTitle, brief: { ...current.workflow.brief, summary: brief }, aspectRatio: current.workflow.aspectRatio, targetDurationMs: current.workflow.targetDurationMs }));
    toast.success('Brief saved');
  });
  const saveConnection = () => void run(async () => {
    if (!connectionProvider || !connectionName.trim() || !apiKey.trim()) throw new Error('Provider, connection name, and API key are required');
    const preset = presets.find((candidate) => candidate.providerId === connectionProvider);
    await api.saveVideoProviderConnection({ providerId: connectionProvider, displayName: connectionName, apiKey, dataRegion: preset?.dataRegions[0] ?? null });
    setApiKey(''); setConnectionName(''); await reload(); toast.success('Encrypted provider connection saved');
  });
  const addShot = () => current && void run(async () => {
    const selected = models[0];
    if (!selected) throw new Error('Add an enabled provider connection first');
    const capability = selected.model.operationCapabilities[0];
    const defaults = defaultDuration(capability);
    replaceWorkflow(await api.addVideoWorkflowShot({ workflowId: current.workflow.id, expectedWorkflowRevision: current.workflow.revision, shot: { title: `Shot ${current.shots.length + 1}`, prompt: '', operation: capability.operation, connectionId: selected.connection.id, providerId: selected.model.providerId, modelId: selected.model.modelId, apiVersion: selected.model.apiVersion, durationSeconds: defaults.durationSeconds, resolution: defaults.resolution, aspectRatio: defaults.aspectRatio, inputAssets: [], seed: null, generateAudio: capability.supportsAudio ? true : null, allowCrossProviderFallback: false } }));
  });
  const moveShot = (shotId: string, direction: -1 | 1) => current && void run(async () => {
    const ids = current.shots.map((shot) => shot.shot.id); const index = ids.indexOf(shotId); const target = index + direction;
    if (index < 0 || target < 0 || target >= ids.length) return;
    [ids[index], ids[target]] = [ids[target], ids[index]];
    replaceWorkflow(await api.reorderVideoWorkflowShots({ workflowId: current.workflow.id, expectedWorkflowRevision: current.workflow.revision, orderedShotIds: ids }));
  });
  const deleteShot = (item: VideoWorkflowShotSnapshot) => current && void run(async () => {
    if (!window.confirm(`Delete “${item.shot.title}”?`)) return;
    replaceWorkflow(await api.deleteVideoWorkflowShot({ workflowId: current.workflow.id, expectedWorkflowRevision: current.workflow.revision, shotId: item.shot.id, expectedShotRevision: item.shot.revision }));
  });
  const reviewQueue = async (item: VideoWorkflowShotSnapshot, count: number) => {
    if (!current) return;
    const idempotencyKey = crypto.randomUUID();
    await run(async () => setPendingQueue({ item, idempotencyKey, disclosure: await api.previewVideoShotQueue({ workflowId: current.workflow.id, expectedWorkflowRevision: current.workflow.revision, shotId: item.shot.id, expectedShotRevision: item.shot.revision, count }) }));
  };
  const confirmQueue = () => pendingQueue && current && void run(async () => {
    const { item, disclosure, idempotencyKey } = pendingQueue;
    replaceWorkflow(await api.queueVideoShotVariants({ workflowId: current.workflow.id, expectedWorkflowRevision: current.workflow.revision, shotId: item.shot.id, expectedShotRevision: item.shot.revision, idempotencyKey, count: disclosure.count, expectedConnectionRevision: disclosure.connectionRevision }));
    setPendingQueue(null);
    toast.success(`${disclosure.count} variant${disclosure.count > 1 ? 's' : ''} queued`);
  });
  const retryVariant = (variant: VideoWorkflowVariantRecord) => void run(async () => replaceWorkflow(await api.retryVideoVariant({ jobId: variant.jobId, expectedJobRevision: variant.job.revision })));
  const cancelVariant = (variant: VideoWorkflowVariantRecord) => void run(async () => {
    if (!window.confirm('Cancel this generation? If completion raced the request, the provider DELETE call may also remove its terminal task record. Local verified output is never deleted by this action.')) return;
    replaceWorkflow(await api.cancelVideoVariant({ jobId: variant.jobId, expectedJobRevision: variant.job.revision, reason: 'Cancelled from Video Studio', allowTerminalRecordDeletion: true }));
  });
  const moveVariant = (shot: VideoWorkflowShotSnapshot, variantId: string, direction: -1 | 1) => current && void run(async () => {
    const ids = shot.variants.map((variant) => variant.id);
    const index = ids.indexOf(variantId); const target = index + direction;
    if (index < 0 || target < 0 || target >= ids.length) return;
    [ids[index], ids[target]] = [ids[target], ids[index]];
    replaceWorkflow(await api.reorderVideoWorkflowVariants({ workflowId: current.workflow.id, expectedWorkflowRevision: current.workflow.revision, shotId: shot.shot.id, expectedShotRevision: shot.shot.revision, orderedVariantIds: ids }));
  });
  const selectVariant = (variant: VideoWorkflowVariantRecord) => current && focusedShot && void run(async () => replaceWorkflow(await api.selectVideoWorkflowVariant({ workflowId: current.workflow.id, expectedWorkflowRevision: current.workflow.revision, shotId: focusedShot.shot.id, expectedShotRevision: focusedShot.shot.revision, variantId: variant.id })));

  if (loading) return <div className="flex h-full items-center justify-center text-sm text-text-tertiary"><Loader2 className="mr-2 h-4 w-4 animate-spin" /> Loading Video Studio</div>;
  return (
    <main className="h-full min-h-0 overflow-y-auto p-4 lg:p-5"><div className="mx-auto flex max-w-[1680px] flex-col gap-4">
      <header className="flex flex-wrap items-center justify-between gap-3"><div><div className="flex items-center gap-2"><Clapperboard className="h-5 w-5 text-accent" /><h1 className="text-lg font-semibold">Video Studio</h1></div><p className="mt-1 text-xs text-text-tertiary">Durable shot planning, provider-backed queueing, and verified local variant selection.</p></div><div className="flex items-center gap-2">{current && <span className="rounded-full border border-border/70 px-2.5 py-1 text-xs">{current.queue.active} active · {formatMicros(current.queue.estimatedCostMicros)}</span>}<Button onClick={() => void reload()} disabled={busy}><RefreshCw className="h-3.5 w-3.5" /> Refresh</Button><Button tone="primary" onClick={createWorkflow} disabled={busy}><Plus className="h-3.5 w-3.5" /> New video</Button></div></header>
      <div className="grid min-h-0 gap-4 xl:grid-cols-[250px_minmax(460px,1fr)_minmax(340px,0.82fr)]">
        <div className="space-y-4">
          <Panel title="Videos"><div className="space-y-1 p-2">{workflows.length === 0 && <p className="px-2 py-5 text-center text-xs text-text-tertiary">Create a video to begin.</p>}{workflows.map((item) => <button key={item.workflow.id} type="button" onClick={() => { setCurrentId(item.workflow.id); setFocusedShotId(item.shots[0]?.shot.id ?? null); }} className={`w-full rounded-lg px-3 py-2 text-left ${current?.workflow.id === item.workflow.id ? 'bg-accent/10 text-accent-hover' : 'text-text-secondary hover:bg-surface-2'}`}><span className="block truncate text-sm font-medium">{item.workflow.title}</span><span className="mt-0.5 block text-[11px] text-text-tertiary">{item.shots.length} shots · {item.queue.active} active</span></button>)}</div></Panel>
          <Panel title="Provider connections"><div className="space-y-2 p-3">{connections.map((connection) => <div key={connection.id} className="flex items-center gap-2 rounded-lg border border-border/60 px-2.5 py-2"><KeyRound className="h-3.5 w-3.5 text-success" /><div className="min-w-0 flex-1"><div className="truncate text-xs font-medium">{connection.displayName}</div><div className="truncate text-[10px] text-text-tertiary">{connection.providerId} · {connection.dataRegion ?? 'default region'}</div></div><button type="button" aria-label={`Delete ${connection.displayName}`} className="text-text-tertiary hover:text-danger" onClick={() => void run(async () => { if (!window.confirm(`Delete connection “${connection.displayName}”? In-use connections remain protected.`)) return; await api.deleteVideoProviderConnection(connection.id, connection.revision); await reload(); })}><Trash2 className="h-3.5 w-3.5" /></button></div>)}<select className={inputClass} value={connectionProvider} onChange={(event) => setConnectionProvider(event.target.value)}><option value="">Provider</option>{presets.filter((preset) => preset.models.some((model) => model.selectable && model.releaseStatus === 'ga')).map((preset) => <option key={preset.id} value={preset.providerId}>{preset.name}</option>)}</select><input className={inputClass} value={connectionName} onChange={(event) => setConnectionName(event.target.value)} placeholder="Connection name" /><input className={inputClass} type="password" autoComplete="new-password" value={apiKey} onChange={(event) => setApiKey(event.target.value)} placeholder="API key (encrypted locally)" /><Button onClick={saveConnection} disabled={busy || !connectionProvider || !connectionName || !apiKey}><KeyRound className="h-3.5 w-3.5" /> Save connection</Button></div></Panel>
        </div>
        <div className="space-y-4">
          {!current ? <Panel title="Shot Board"><div className="p-10 text-center text-sm text-text-tertiary">Create a video, connect a provider, then arrange its shots.</div></Panel> : <><Panel title="Brief" actions={<Button onClick={saveWorkflow} disabled={busy}><Save className="h-3.5 w-3.5" /> Save</Button>}><div className="grid gap-3 p-4 md:grid-cols-[minmax(180px,0.42fr)_1fr]"><input className={inputClass} value={workflowTitle} onChange={(event) => setWorkflowTitle(event.target.value)} aria-label="Video title" /><textarea className={`${areaClass} min-h-16 resize-y`} value={brief} onChange={(event) => setBrief(event.target.value)} placeholder="Creative brief, audience, tone, and constraints" /></div></Panel><Panel title="Shot Board" actions={<Button tone="primary" onClick={addShot} disabled={busy || models.length === 0}><Plus className="h-3.5 w-3.5" /> Add shot</Button>}><div className="space-y-3 p-3">{models.length === 0 && <div className="rounded-lg border border-warning/30 bg-warning/5 p-3 text-xs text-text-secondary">Add a connection for a GA model. Preview, announced, and contract-pending models are never selectable.</div>}{current.shots.length === 0 && <p className="py-10 text-center text-sm text-text-tertiary">The board is empty. Add a shot to create the first durable node.</p>}{current.shots.map((item) => <ShotCard key={item.shot.id} item={item} workflowRevision={current.workflow.revision} models={models} busy={busy} focused={focusedShot?.shot.id === item.shot.id} onFocus={() => setFocusedShotId(item.shot.id)} onChange={replaceWorkflow} onQueue={(count) => reviewQueue(item, count)} onMove={(direction) => moveShot(item.shot.id, direction)} onDelete={() => deleteShot(item)} />)}</div></Panel></>}
        </div>
        <div className="space-y-4">
          <Panel title="Generation Queue" actions={activeJobs > 0 ? <span className="inline-flex items-center gap-1 text-[11px] text-accent-hover"><Loader2 className="h-3 w-3 animate-spin" /> observing</span> : undefined}>
            <div className="space-y-2 p-3">
              {(!current || current.shots.every((shot) => shot.variants.length === 0)) && <p className="py-8 text-center text-xs text-text-tertiary">Queued variants project the durable media job queue.</p>}
              {current?.shots.flatMap((shot) => shot.variants.map((variant) => ({ shot, variant }))).map(({ shot, variant }) => (
                <div key={variant.id} className="rounded-lg border border-border/60 bg-surface-0 p-3">
                  <div className="flex items-start gap-2"><span className={`rounded-full border px-2 py-0.5 text-[10px] font-semibold uppercase ${variantTone(variant)}`}>{variantStatusLabel(variant)}</span><div className="min-w-0 flex-1"><div className="truncate text-xs font-medium">{shot.shot.title} · {variant.label}</div><div className="mt-0.5 text-[10px] text-text-tertiary">{variant.job.providerId}/{variant.job.modelId} · {formatMicros(variant.job.finalCostMicros ?? variant.job.estimatedCostMicros, variant.job.currency)}</div></div></div>
                  {variant.job.error && <p className="mt-2 line-clamp-2 text-[11px] text-warning">{String(variant.job.error.message ?? variant.job.error.code ?? 'Provider error')}{variant.job.nextEligibleAt ? ` · retry after ${new Date(variant.job.nextEligibleAt).toLocaleTimeString()}` : ''}</p>}
                  <div className="mt-2 flex justify-end gap-2"><Button onClick={() => moveVariant(shot, variant.id, -1)} disabled={busy || variant.ordinal === 0} title="Move variant earlier"><ArrowUp className="h-3.5 w-3.5" /></Button><Button onClick={() => moveVariant(shot, variant.id, 1)} disabled={busy || variant.ordinal === shot.variants.length - 1} title="Move variant later"><ArrowDown className="h-3.5 w-3.5" /></Button>{mayRetryVariant(variant) && <Button onClick={() => retryVariant(variant)} disabled={busy}><RotateCcw className="h-3.5 w-3.5" /> {variant.job.state === 'provider_unknown' ? 'Recheck' : variant.job.state === 'post_processing' ? 'Retry output' : 'Retry'}</Button>}{mayCancelVariant(variant) && <Button onClick={() => cancelVariant(variant)} disabled={busy} tone="danger"><X className="h-3.5 w-3.5" /> Cancel</Button>}</div>
                </div>
              ))}
            </div>
          </Panel>
          <Panel title={`Compare${focusedShot ? ` · ${focusedShot.shot.title}` : ''}`}>
            <div className="p-3"><SynchronizedCompare variants={focusedShot?.variants.filter((variant) => variant.job.state === 'completed' && variant.job.outputAssetId != null) ?? []} selectedId={focusedShot?.shot.selectedVariantId ?? null} busy={busy} onSelect={selectVariant} /></div>
          </Panel>
          {focusedShot && <Panel title="Privacy & provider contract"><dl className="grid grid-cols-[auto_1fr] gap-x-3 gap-y-2 p-3 text-xs"><dt className="text-text-tertiary">Target</dt><dd>{focusedShot.shot.providerId}/{focusedShot.shot.modelId}</dd><dt className="text-text-tertiary">API</dt><dd>{focusedShot.shot.apiVersion ?? 'Provider default'}</dd><dt className="text-text-tertiary">Region</dt><dd>{focusedShot.shot.dataRegion ?? 'Provider default'}</dd><dt className="text-text-tertiary">Retention</dt><dd>{focusedShot.shot.retentionPolicy}</dd><dt className="text-text-tertiary">Deletion</dt><dd>{focusedModel ? `${focusedModel.cancellationScope}; terminal record risk ${focusedModel.cancellationMayDeleteTerminalRecord ? 'requires consent' : 'none declared'}` : 'Unverified'}</dd><dt className="text-text-tertiary">Watermark</dt><dd>{focusedShot.shot.watermarkPolicy}</dd><dt className="text-text-tertiary">Provenance</dt><dd>{focusedShot.shot.provenancePolicy}</dd><dt className="text-text-tertiary">Fallback</dt><dd className="font-medium text-success">Off — no silent forwarding</dd></dl></Panel>}
          {current && <Panel title="Typed DAG"><div className="space-y-1 p-3">{current.dag.nodes.map((node) => <div key={node.id} className="flex items-center gap-2 rounded-md border border-border/50 px-2 py-1.5 text-[11px]"><span className="rounded bg-surface-3 px-1.5 py-0.5 font-medium">{node.kind.replace(/_/g, ' ')}</span><span className="min-w-0 flex-1 truncate text-text-secondary">{node.id}</span><span className="text-text-tertiary">{node.dependsOn.length} deps</span></div>)}</div></Panel>}
        </div>
      </div>
      {pendingQueue && <div className="fixed inset-0 z-50 flex items-center justify-center bg-black/60 p-4" role="dialog" aria-modal="true" aria-label="Review queue disclosure"><div className="w-full max-w-2xl rounded-xl border border-border bg-surface-1 shadow-xl"><header className="flex items-center justify-between border-b border-border px-4 py-3"><div><h2 className="text-sm font-semibold">Review provider transfer & cost</h2><p className="mt-0.5 text-xs text-text-tertiary">Nothing is submitted until you confirm this exact shot, connection, and input digest.</p></div><Button onClick={() => setPendingQueue(null)}><X className="h-3.5 w-3.5" /></Button></header><dl className="grid max-h-[65vh] grid-cols-[auto_1fr] gap-x-4 gap-y-2 overflow-y-auto p-4 text-xs"><dt className="text-text-tertiary">Provider/model</dt><dd>{pendingQueue.disclosure.providerId}/{pendingQueue.disclosure.modelId} · API {pendingQueue.disclosure.apiVersion ?? 'default'}</dd><dt className="text-text-tertiary">Endpoint</dt><dd className="break-all">{pendingQueue.disclosure.officialBaseUrl}</dd><dt className="text-text-tertiary">Account scope</dt><dd>{pendingQueue.disclosure.connectionName} · {pendingQueue.disclosure.credentialScope} · revision {pendingQueue.disclosure.connectionRevision}</dd><dt className="text-text-tertiary">Region</dt><dd>{pendingQueue.disclosure.dataRegion ?? 'Provider default'}</dd><dt className="text-text-tertiary">Retention</dt><dd>{pendingQueue.disclosure.retentionPolicy}</dd><dt className="text-text-tertiary">Deletion</dt><dd>{pendingQueue.disclosure.deletionPolicy}</dd><dt className="text-text-tertiary">Ordered inputs</dt><dd>{pendingQueue.disclosure.orderedInputs.length === 0 ? 'Text prompt only' : pendingQueue.disclosure.orderedInputs.map((input) => `${input.ordinal + 1}. ${input.role} (${input.mediaType}, ${input.byteLength ?? 'unknown'} bytes, sha256 ${input.contentHashSha256.slice(0, 12)}…, ${input.uri})`).join('; ')}</dd><dt className="text-text-tertiary">Estimated cost</dt><dd className="font-semibold">{formatMicros(pendingQueue.disclosure.estimatedCostMicrosTotal, pendingQueue.disclosure.currency)} total · {formatMicros(pendingQueue.disclosure.estimatedCostMicrosPerVariant, pendingQueue.disclosure.currency)} × {pendingQueue.disclosure.count}</dd><dt className="text-text-tertiary">Fallback</dt><dd className="font-medium text-success">Off — inputs will not be forwarded to another provider</dd></dl><footer className="flex justify-end gap-2 border-t border-border px-4 py-3"><Button onClick={() => setPendingQueue(null)}>Back</Button><Button tone="primary" onClick={confirmQueue} disabled={busy}><Play className="h-3.5 w-3.5" /> Confirm & queue</Button></footer></div></div>}
    </div></main>
  );
}
