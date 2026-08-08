import { listen } from '@tauri-apps/api/event';
import { getCurrentWindow } from '@tauri-apps/api/window';
import { useCallback, useEffect, useMemo, useRef, useState } from 'react';

import { DEFAULT_COMPANION_SETTINGS } from '../features/companion/defaults';
import * as api from '../lib/api';
import type { CompanionSettings } from '../types/conversation';

const ANIMATION_CANDIDATES: Record<api.CompanionState, string[]> = {
  idle: ['idle'],
  thinking: ['thinking', 'running', 'review', 'idle'],
  searching: ['searching', 'running', 'moveRight', 'idle'],
  browsing: ['browsing', 'running', 'moveRight', 'idle'],
  readingFiles: ['readingFiles', 'review', 'running', 'idle'],
  runningTool: ['runningTool', 'running', 'idle'],
  coding: ['coding', 'running', 'review', 'idle'],
  waitingForApproval: ['waitingForApproval', 'waiting', 'idle'],
  waitingForUser: ['waitingForUser', 'waiting', 'waving', 'idle'],
  reviewing: ['reviewing', 'review', 'idle'],
  succeeded: ['succeeded', 'waving', 'jumping', 'idle'],
  failed: ['failed', 'idle'],
  cancelled: ['cancelled', 'failed', 'idle'],
  sleeping: ['sleeping', 'idle'],
};

function selectAnimation(
  pack: api.NormalizedCompanionPack | null,
  state: api.CompanionState,
): api.NormalizedCompanionAnimation | null {
  if (!pack) return null;
  for (const candidate of ANIMATION_CANDIDATES[state]) {
    if (pack.animations[candidate]) return pack.animations[candidate];
  }
  return Object.values(pack.animations)[0] ?? null;
}

function Sprite({
  asset,
  pack,
  projection,
  settings,
  active,
}: {
  asset: string | null;
  pack: api.NormalizedCompanionPack | null;
  projection: api.CompanionProjection | null;
  settings: CompanionSettings;
  active: boolean;
}) {
  const state = projection?.state ?? 'idle';
  const animation = useMemo(() => selectAnimation(pack, state), [pack, state]);
  const [frameIndex, setFrameIndex] = useState(0);

  useEffect(() => {
    setFrameIndex(0);
    if (!active || !animation || settings.reducedMotion || animation.frames.length <= 1) return;
    const fps = Math.min(animation.fps, settings.animationFpsCap);
    const timer = window.setInterval(() => {
      setFrameIndex((current) => {
        const next = current + 1;
        if (next < animation.frames.length) return next;
        return animation.looping ? 0 : current;
      });
    }, Math.max(16, Math.round(1_000 / fps)));
    return () => window.clearInterval(timer);
  }, [active, animation, settings.animationFpsCap, settings.reducedMotion]);

  if (!asset || !pack || !animation) {
    return (
      <div className={`companion-fallback companion-fallback--${state}`} aria-label="Nexa Desktop Pet">
        <span className="companion-fallback__ear companion-fallback__ear--left" />
        <span className="companion-fallback__ear companion-fallback__ear--right" />
        <span className="companion-fallback__eye companion-fallback__eye--left" />
        <span className="companion-fallback__eye companion-fallback__eye--right" />
        <span className="companion-fallback__mouth" />
      </div>
    );
  }

  const frame = animation.frames[Math.min(frameIndex, animation.frames.length - 1)] ?? 0;
  const column = frame % pack.frame.columns;
  const row = Math.floor(frame / pack.frame.columns);
  const x = pack.frame.columns <= 1 ? 0 : (column / (pack.frame.columns - 1)) * 100;
  const y = pack.frame.rows <= 1 ? 0 : (row / (pack.frame.rows - 1)) * 100;
  const aspect = pack.frame.width / pack.frame.height;

  return (
    <div
      className="companion-sprite"
      aria-label={pack.displayName}
      style={{
        aspectRatio: `${aspect}`,
        backgroundImage: `url(${asset})`,
        backgroundPosition: `${x}% ${y}%`,
        backgroundSize: `${pack.frame.columns * 100}% ${pack.frame.rows * 100}%`,
      }}
    />
  );
}

export function CompanionWindowPage() {
  const [settings, setSettings] = useState<CompanionSettings>(DEFAULT_COMPANION_SETTINGS);
  const [pack, setPack] = useState<api.NormalizedCompanionPack | null>(null);
  const [asset, setAsset] = useState<string | null>(null);
  const [projection, setProjection] = useState<api.CompanionProjection | null>(null);
  const [visible, setVisible] = useState(false);
  const refreshTimer = useRef<number | null>(null);

  const refreshProjection = useCallback(async () => {
    const next = await api.getGlobalCompanionProjection().catch(() => null);
    setProjection(next);
  }, []);

  const scheduleProjectionRefresh = useCallback(() => {
    if (refreshTimer.current !== null) window.clearTimeout(refreshTimer.current);
    refreshTimer.current = window.setTimeout(() => {
      refreshTimer.current = null;
      void refreshProjection();
    }, 250);
  }, [refreshProjection]);

  const loadRuntime = useCallback(async () => {
    const [config, catalog] = await Promise.all([
      api.getAppConfig(),
      api.scanCompanionPacks(),
    ]);
    const nextSettings = config.companion ?? DEFAULT_COMPANION_SETTINGS;
    const nextPack = catalog.packs.find((candidate) => candidate.id === nextSettings.selectedPetId)
      ?? catalog.packs[0]
      ?? null;
    setSettings(nextSettings);
    setPack(nextPack);
    setAsset(null);
    if (nextPack) {
      const nextAsset = await api.readCompanionAsset(nextPack.id, nextPack.contentHash);
      setAsset(nextAsset.dataUrl);
    }
  }, []);

  useEffect(() => {
    document.documentElement.dataset.companionWindow = 'true';
    void Promise.all([loadRuntime(), refreshProjection()]).finally(() => {
      window.requestAnimationFrame(() => { void api.companionRendererReady(); });
    });
    return () => {
      delete document.documentElement.dataset.companionWindow;
    };
  }, [loadRuntime, refreshProjection]);

  useEffect(() => {
    const unlisten = Promise.all([
      listen('agent:event', scheduleProjectionRefresh),
      listen('companion://settings-changed', () => { void loadRuntime(); }),
      listen<boolean>('companion://visibility', (event) => setVisible(event.payload)),
    ]);
    return () => {
      if (refreshTimer.current !== null) window.clearTimeout(refreshTimer.current);
      void unlisten.then((callbacks) => callbacks.forEach((callback) => callback()));
    };
  }, [loadRuntime, refreshProjection, scheduleProjectionRefresh]);

  useEffect(() => {
    if (!settings.enabled || settings.displayMode !== 'during_tasks') return;
    if (projection && !projection.terminal) {
      void api.showCompanion();
      return;
    }
    const hold = projection?.state === 'failed' ? settings.failureHoldMs : settings.successHoldMs;
    const timer = window.setTimeout(() => { void api.hideCompanion(); }, projection ? hold : 0);
    return () => window.clearTimeout(timer);
  }, [
    projection?.runId,
    projection?.state,
    projection?.terminal,
    settings.displayMode,
    settings.enabled,
    settings.failureHoldMs,
    settings.successHoldMs,
  ]);

  useEffect(() => {
    let persistTimer: number | null = null;
    const unlisten = getCurrentWindow().onMoved(() => {
      if (persistTimer !== null) window.clearTimeout(persistTimer);
      persistTimer = window.setTimeout(() => { void api.persistCompanionPosition(); }, 250);
    });
    return () => {
      if (persistTimer !== null) window.clearTimeout(persistTimer);
      void unlisten.then((callback) => callback());
    };
  }, []);

  const startDrag = (event: React.PointerEvent) => {
    if (event.button !== 0 || settings.lockPosition || settings.interactionMode !== 'smart') return;
    void getCurrentWindow().startDragging();
  };

  const label = settings.privacyMode
    ? projection?.terminal ? 'Task finished' : projection ? 'Nexa is working' : 'Ready'
    : projection?.label ?? 'Ready';

  return (
    <main
      className="companion-window-root"
      data-state={projection?.state ?? 'idle'}
      data-visible={visible}
      onPointerDown={startDrag}
      onDoubleClick={() => { void api.executeCompanionCommand('settings'); }}
      style={{ transform: `scale(${settings.scale})` }}
    >
      {settings.showBubbles && projection && (
        <div className="companion-bubble" role="status">{label}</div>
      )}
      <Sprite asset={asset} pack={pack} projection={projection} settings={settings} active={visible} />
      <div className="companion-shadow" aria-hidden="true" />
    </main>
  );
}
