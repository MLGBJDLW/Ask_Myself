import { PhysicalPosition } from '@tauri-apps/api/dpi';
import { listen } from '@tauri-apps/api/event';
import { currentMonitor, cursorPosition, getCurrentWindow } from '@tauri-apps/api/window';
import {
  useCallback,
  useEffect,
  useMemo,
  useReducer,
  useRef,
  useState,
} from 'react';

import { DEFAULT_COMPANION_SETTINGS } from '../features/companion/defaults';
import {
  decodeImageSource,
  classifyDragDirection,
  reduceCompanionBehavior,
  resolveAnimationFrame,
  resolveDragDirection,
  resolveLookDirection,
  resolveWalkStep,
  selectCompanionAnimation,
  taskBehavior,
  type CompanionBehaviorState,
} from '../features/companion/runtime';
import { useTranslation } from '../i18n';
import * as api from '../lib/api';
import type { CompanionSettings } from '../types/conversation';

const STATE_HYSTERESIS_MS = 180;
const DRAG_THRESHOLD_PX = 6;
const CLICK_SEQUENCE_MS = 850;
const IDLE_GESTURE_DELAY_MS = 8_000;
const AUTO_WALK_DELAY_MS = 6_000;
const AUTO_WALK_DURATION_MS = 4_000;
const POSITION_WRITE_INTERVAL_MS = 34;
const DRAG_DIRECTION_POLL_MS = 34;
const LOOK_DIRECTION_POLL_MS = 100;
const LOOK_DIRECTION_DEAD_ZONE_PX = 36;

interface DecodedCompanionRuntime {
  pack: api.NormalizedCompanionPack;
  asset: string;
}

const SINGLE_PASS_BEHAVIORS = new Set<CompanionBehaviorState>([
  'beingPetted',
  'clicked',
  'dropped',
  'waving',
  'reactingToSuccess',
  'reactingToFailure',
]);

function Sprite({
  runtime,
  state,
  behavior,
  settings,
  reducedMotion,
  active,
  terminal,
  lookDirection,
  onAnimationComplete,
}: {
  runtime: DecodedCompanionRuntime | null;
  state: api.CompanionState;
  behavior: CompanionBehaviorState;
  settings: CompanionSettings;
  reducedMotion: boolean;
  active: boolean;
  terminal: boolean;
  lookDirection: number | null;
  onAnimationComplete: () => void;
}) {
  const selected = useMemo(() => {
    if (runtime?.pack.dialect === 'codex_desktop_v2' && lookDirection !== null) {
      const key = `look${lookDirection}`;
      const animation = runtime.pack.animations[key];
      if (animation) return { key, animation };
    }
    return selectCompanionAnimation(runtime?.pack ?? null, state, behavior);
  }, [behavior, lookDirection, runtime?.pack, state]);
  const [frameIndex, setFrameIndex] = useState(0);
  const completionRef = useRef(onAnimationComplete);
  completionRef.current = onAnimationComplete;
  const singlePass = terminal || SINGLE_PASS_BEHAVIORS.has(behavior);
  const playbackKey = runtime && selected
    ? `${runtime.pack.contentHash}:${selected.key}:${singlePass ? 'once' : 'loop'}`
    : 'fallback';

  useEffect(() => {
    setFrameIndex(0);
  }, [playbackKey]);

  useEffect(() => {
    if (!active || !selected || reducedMotion || selected.animation.frames.length <= 1) {
      return;
    }
    const fps = Math.max(1, Math.min(selected.animation.fps, settings.animationFpsCap));
    const looping = selected.animation.looping && !singlePass;
    const startedAt = performance.now();
    let animationFrame = 0;
    let wakeTimer = 0;
    let completionSent = false;
    const frameDurationMs = 1_000 / fps;
    const tick = (now: number) => {
      const next = resolveAnimationFrame(
        now - startedAt,
        fps,
        selected.animation.frames.length,
        looping,
      );
      setFrameIndex(current => current === next.index ? current : next.index);
      if (next.completed) {
        if (!completionSent) {
          completionSent = true;
          completionRef.current();
        }
        return;
      }
      const elapsed = Math.max(0, now - startedAt);
      const nextBoundary = (Math.floor(elapsed / frameDurationMs) + 1) * frameDurationMs;
      const waitMs = Math.max(0, nextBoundary - elapsed);
      if (waitMs > 20) {
        wakeTimer = window.setTimeout(() => {
          animationFrame = window.requestAnimationFrame(tick);
        }, Math.max(0, waitMs - 8));
      } else {
        animationFrame = window.requestAnimationFrame(tick);
      }
    };
    animationFrame = window.requestAnimationFrame(tick);
    return () => {
      window.clearTimeout(wakeTimer);
      window.cancelAnimationFrame(animationFrame);
    };
  }, [active, playbackKey, reducedMotion, selected, settings.animationFpsCap, singlePass]);

  if (!runtime || !selected) {
    return (
      <div
        className={`companion-fallback companion-fallback--${state}`}
        aria-label="Nexa Desktop Pet"
      >
        <span className="companion-fallback__ear companion-fallback__ear--left" />
        <span className="companion-fallback__ear companion-fallback__ear--right" />
        <span className="companion-fallback__eye companion-fallback__eye--left" />
        <span className="companion-fallback__eye companion-fallback__eye--right" />
        <span className="companion-fallback__mouth" />
      </div>
    );
  }

  const frame = selected.animation.frames[
    Math.min(frameIndex, selected.animation.frames.length - 1)
  ] ?? 0;
  const column = frame % runtime.pack.frame.columns;
  const row = Math.floor(frame / runtime.pack.frame.columns);
  const x = runtime.pack.frame.columns <= 1
    ? 0
    : (column / (runtime.pack.frame.columns - 1)) * 100;
  const y = runtime.pack.frame.rows <= 1
    ? 0
    : (row / (runtime.pack.frame.rows - 1)) * 100;
  const aspect = runtime.pack.frame.width / runtime.pack.frame.height;

  return (
    <div
      className="companion-sprite"
      aria-label={runtime.pack.displayName}
      data-animation={selected.key}
      style={{
        aspectRatio: `${aspect}`,
        backgroundImage: `url(${runtime.asset})`,
        backgroundPosition: `${x}% ${y}%`,
        backgroundSize: `${runtime.pack.frame.columns * 100}% ${runtime.pack.frame.rows * 100}%`,
      }}
    />
  );
}

export function CompanionWindowPage() {
  const { t } = useTranslation();
  const [settings, setSettings] = useState<CompanionSettings>(DEFAULT_COMPANION_SETTINGS);
  const [runtime, setRuntime] = useState<DecodedCompanionRuntime | null>(null);
  const [runtimeLoaded, setRuntimeLoaded] = useState(false);
  const [projection, setProjection] = useState<api.CompanionProjection | null>(null);
  const [stableProjection, setStableProjection] = useState<api.CompanionProjection | null>(null);
  const [behavior, dispatchBehavior] = useReducer(reduceCompanionBehavior, 'idle');
  const [terminalAnimationConsumed, setTerminalAnimationConsumed] = useState(false);
  const [contextMenuOpen, setContextMenuOpen] = useState(false);
  const [visible, setVisible] = useState(false);
  const [systemReducedMotion, setSystemReducedMotion] = useState(false);
  const [mainWindowVisible, setMainWindowVisible] = useState<boolean | null>(null);
  const [lookDirection, setLookDirection] = useState<number | null>(null);
  const [pageVisible, setPageVisible] = useState(() => document.visibilityState !== 'hidden');
  const refreshTimer = useRef<number | null>(null);
  const runtimeRequest = useRef(0);
  const runtimeRef = useRef<DecodedCompanionRuntime | null>(null);
  const rendererReadySent = useRef(false);
  const pointer = useRef<{
    id: number;
    x: number;
    y: number;
    dragging: boolean;
    direction: 'left' | 'right';
    lastCursor: { x: number; y: number } | null;
  } | null>(null);
  const clickTimes = useRef<number[]>([]);
  const nextIdleAction = useRef<'gesture' | 'walk'>('walk');

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
    const request = ++runtimeRequest.current;
    try {
      const [config, catalog] = await Promise.all([
        api.getAppConfig(),
        api.scanCompanionPacks(),
      ]);
      if (runtimeRequest.current !== request) return;
      const nextSettings = config.companion ?? DEFAULT_COMPANION_SETTINGS;
      const nextPack = catalog.packs.find(candidate => candidate.id === nextSettings.selectedPetId)
        ?? catalog.packs[0]
        ?? null;
      setSettings(nextSettings);
      if (!nextPack) {
        // Keep an already committed runtime visible. On first launch there is
        // no decoded frame to show, so leave the native window hidden.
        if (runtimeRef.current) setRuntimeLoaded(true);
        return;
      }
      const currentRuntime = runtimeRef.current;
      if (
        currentRuntime?.pack.id === nextPack.id
        && currentRuntime.pack.contentHash === nextPack.contentHash
      ) {
        setRuntimeLoaded(true);
        return;
      }
      const nextAsset = await api.readCompanionAsset(nextPack.id, nextPack.contentHash);
      await decodeImageSource(nextAsset.dataUrl);
      if (runtimeRequest.current !== request) return;
      const decodedRuntime = { pack: nextPack, asset: nextAsset.dataUrl };
      runtimeRef.current = decodedRuntime;
      setRuntime(decodedRuntime);
      setRuntimeLoaded(true);
    } catch (error) {
      console.error('[companion] failed to load decoded runtime', error);
      // A failed refresh must not replace a committed pet. A failed first
      // decode must not announce readiness and expose the fallback surface.
      if (runtimeRequest.current === request && runtimeRef.current) {
        setRuntimeLoaded(true);
      }
    }
  }, []);

  useEffect(() => {
    document.documentElement.dataset.companionWindow = 'true';
    void loadRuntime();
    void refreshProjection();
    return () => {
      runtimeRequest.current += 1;
      delete document.documentElement.dataset.companionWindow;
    };
  }, [loadRuntime, refreshProjection]);

  useEffect(() => {
    const query = window.matchMedia('(prefers-reduced-motion: reduce)');
    const update = () => setSystemReducedMotion(query.matches);
    update();
    query.addEventListener('change', update);
    return () => query.removeEventListener('change', update);
  }, []);

  useEffect(() => {
    const update = () => setPageVisible(document.visibilityState !== 'hidden');
    document.addEventListener('visibilitychange', update);
    return () => document.removeEventListener('visibilitychange', update);
  }, []);

  useEffect(() => {
    if (!runtimeLoaded || rendererReadySent.current) return;
    let outerFrame = 0;
    let innerFrame = 0;
    outerFrame = window.requestAnimationFrame(() => {
      innerFrame = window.requestAnimationFrame(() => {
        rendererReadySent.current = true;
        void api.companionRendererReady();
      });
    });
    return () => {
      window.cancelAnimationFrame(outerFrame);
      window.cancelAnimationFrame(innerFrame);
    };
  }, [runtimeLoaded]);

  useEffect(() => {
    if (!projection || !stableProjection || projection.terminal) {
      setStableProjection(projection);
      return;
    }
    if (
      projection.runId === stableProjection.runId
      && projection.state === stableProjection.state
      && projection.terminal === stableProjection.terminal
    ) {
      setStableProjection(projection);
      return;
    }
    const timer = window.setTimeout(
      () => setStableProjection(projection),
      STATE_HYSTERESIS_MS,
    );
    return () => window.clearTimeout(timer);
  }, [projection, stableProjection]);

  useEffect(() => {
    setTerminalAnimationConsumed(false);
  }, [stableProjection?.runId, stableProjection?.state, stableProjection?.terminal]);

  useEffect(() => {
    const unlisten = Promise.all([
      listen('companion://projection-changed', scheduleProjectionRefresh),
      listen('companion://settings-changed', () => { void loadRuntime(); }),
      listen<boolean>('companion://visibility', event => setVisible(event.payload)),
      listen<boolean>('companion://main-visibility', event => setMainWindowVisible(event.payload)),
    ]);
    return () => {
      if (refreshTimer.current !== null) window.clearTimeout(refreshTimer.current);
      void unlisten.then(callbacks => callbacks.forEach(callback => callback()));
    };
  }, [loadRuntime, scheduleProjectionRefresh]);

  useEffect(() => {
    if (!settings.enabled) return;
    if (mainWindowVisible === false && !settings.continueWhenMainHidden) {
      void api.hideCompanion();
      return;
    }
    if (settings.displayMode === 'always') {
      if (mainWindowVisible === true && !settings.continueWhenMainHidden) {
        void api.showCompanion();
      }
      return;
    }
    if (settings.displayMode !== 'during_tasks') return;
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
    mainWindowVisible,
    settings.continueWhenMainHidden,
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
      void unlisten.then(callback => callback());
    };
  }, []);

  const stableState = terminalAnimationConsumed ? 'idle' : stableProjection?.state ?? 'idle';
  const effectiveBehavior = behavior === 'idle' ? taskBehavior(stableState) : behavior;
  const reducedMotion = settings.reducedMotion || systemReducedMotion;
  const motionActive = visible && pageVisible;

  useEffect(() => {
    const canTrack = runtime?.pack.dialect === 'codex_desktop_v2'
      && motionActive
      && !reducedMotion
      && stableState === 'idle'
      && (behavior === 'idle' || behavior === 'hovering');
    if (!canTrack) {
      setLookDirection(null);
      return;
    }
    let cancelled = false;
    let timer = 0;
    const companionWindow = getCurrentWindow();
    const sample = async () => {
      try {
        const [pointerPosition, windowPosition, windowSize] = await Promise.all([
          cursorPosition(),
          companionWindow.outerPosition(),
          companionWindow.outerSize(),
        ]);
        if (cancelled) return;
        setLookDirection(current => resolveLookDirection(
          pointerPosition,
          {
            x: windowPosition.x + windowSize.width / 2,
            y: windowPosition.y + windowSize.height / 2,
          },
          LOOK_DIRECTION_DEAD_ZONE_PX,
          current,
        ));
      } catch {
        if (!cancelled) setLookDirection(null);
      }
      if (!cancelled) timer = window.setTimeout(() => { void sample(); }, LOOK_DIRECTION_POLL_MS);
    };
    void sample();
    return () => {
      cancelled = true;
      window.clearTimeout(timer);
    };
  }, [behavior, motionActive, reducedMotion, runtime?.pack.dialect, stableState]);

  useEffect(() => {
    if (reducedMotion || !motionActive || stableState !== 'idle' || behavior !== 'idle') return;
    const canGesture = settings.idleActions;
    const canWalk = settings.autoWalk
      && !settings.lockPosition
      && settings.interactionMode === 'smart';
    if (!canGesture && !canWalk) return;

    const action = canGesture && canWalk
      ? nextIdleAction.current
      : canWalk ? 'walk' : 'gesture';
    if (canGesture && canWalk) {
      nextIdleAction.current = action === 'walk' ? 'gesture' : 'walk';
    }
    const timer = window.setTimeout(() => {
      if (action === 'walk') {
        dispatchBehavior({
          type: 'walkStarted',
          direction: Date.now() % 2 === 0 ? 'left' : 'right',
        });
      } else {
        dispatchBehavior({
          type: 'idleGesture',
          gesture: Date.now() % 3 === 0 ? 'sleep' : 'wave',
        });
      }
    }, action === 'walk' ? AUTO_WALK_DELAY_MS : IDLE_GESTURE_DELAY_MS);
    return () => window.clearTimeout(timer);
  }, [
    behavior,
    reducedMotion,
    settings.autoWalk,
    settings.idleActions,
    settings.interactionMode,
    settings.lockPosition,
    stableState,
    motionActive,
  ]);

  useEffect(() => {
    if (behavior !== 'walkingLeft' && behavior !== 'walkingRight') return;
    if (
      !settings.autoWalk
      || reducedMotion
      || !motionActive
      || settings.lockPosition
      || settings.interactionMode !== 'smart'
      || stableState !== 'idle'
    ) {
      dispatchBehavior({ type: 'animationCompleted' });
      return;
    }
    let cancelled = false;
    let animationFrame = 0;
    const direction = behavior === 'walkingLeft' ? 'left' : 'right';
    const companionWindow = getCurrentWindow();
    void Promise.all([
      companionWindow.outerPosition(),
      companionWindow.outerSize(),
      currentMonitor(),
    ]).then(([position, size, monitor]) => {
      if (cancelled || !monitor) {
        dispatchBehavior({ type: 'animationCompleted' });
        return;
      }
      let x = position.x;
      let walkDirection: 'left' | 'right' = direction;
      let lastFrameAt = performance.now();
      let lastPositionWriteAt = lastFrameAt;
      let positionWritePending = false;
      const startedAt = lastFrameAt;
      const bounds = {
        minX: monitor.workArea.position.x + 12,
        maxX: monitor.workArea.position.x + monitor.workArea.size.width - size.width - 12,
      };
      const tick = (now: number) => {
        if (cancelled) return;
        const next = resolveWalkStep(x, walkDirection, now - lastFrameAt, bounds);
        x = next.x;
        walkDirection = next.direction;
        lastFrameAt = now;
        if (!positionWritePending && now - lastPositionWriteAt >= POSITION_WRITE_INTERVAL_MS) {
          lastPositionWriteAt = now;
          positionWritePending = true;
          void companionWindow
            .setPosition(new PhysicalPosition(Math.round(x), position.y))
            .finally(() => { positionWritePending = false; });
        }
        if (next.turned) {
          dispatchBehavior({ type: 'walkTurned', direction: next.direction });
          return;
        }
        if (now - startedAt >= AUTO_WALK_DURATION_MS) {
          dispatchBehavior({ type: 'animationCompleted' });
          void api.persistCompanionPosition();
          return;
        }
        animationFrame = window.requestAnimationFrame(tick);
      };
      animationFrame = window.requestAnimationFrame(tick);
    }).catch(() => dispatchBehavior({ type: 'animationCompleted' }));
    return () => {
      cancelled = true;
      window.cancelAnimationFrame(animationFrame);
    };
  }, [
    behavior,
    reducedMotion,
    settings.autoWalk,
    settings.interactionMode,
    settings.lockPosition,
    stableState,
    motionActive,
  ]);

  useEffect(() => {
    const duration = behavior === 'sleeping' ? 2_400 : 1_100;
    if (!['beingPetted', 'clicked', 'dropped', 'waving', 'sleeping'].includes(behavior)) return;
    const timer = window.setTimeout(
      () => dispatchBehavior({ type: 'animationCompleted' }),
      duration,
    );
    return () => window.clearTimeout(timer);
  }, [behavior]);

  const beginPointer = (event: React.PointerEvent<HTMLElement>) => {
    if (event.button !== 0 || settings.interactionMode === 'click_through') return;
    pointer.current = {
      id: event.pointerId,
      x: event.clientX,
      y: event.clientY,
      dragging: false,
      direction: behavior === 'walkingLeft' || behavior === 'draggingLeft' ? 'left' : 'right',
      lastCursor: null,
    };
    event.currentTarget.setPointerCapture(event.pointerId);
  };

  const movePointer = (event: React.PointerEvent<HTMLElement>) => {
    const pressed = pointer.current;
    if (
      !pressed
      || pressed.id !== event.pointerId
      || pressed.dragging
      || settings.lockPosition
      || settings.interactionMode !== 'smart'
    ) return;
    if (Math.hypot(event.clientX - pressed.x, event.clientY - pressed.y) < DRAG_THRESHOLD_PX) return;
    pressed.dragging = true;
    pressed.direction = resolveDragDirection(
      event.clientX - pressed.x,
      event.clientY - pressed.y,
      pressed.direction,
      DRAG_THRESHOLD_PX,
    );
    dispatchBehavior({ type: 'dragStarted', direction: pressed.direction });
    let directionTimer = 0;
    const sampleDragDirection = async () => {
      const active = pointer.current;
      if (active !== pressed || !active.dragging) return;
      try {
        const nextCursor = await cursorPosition();
        if (active.lastCursor) {
          const nextDirection = classifyDragDirection(
            nextCursor.x - active.lastCursor.x,
            nextCursor.y - active.lastCursor.y,
          );
          if (nextDirection) {
            active.lastCursor = nextCursor;
            if (nextDirection !== active.direction) {
              active.direction = nextDirection;
              dispatchBehavior({ type: 'dragMoved', direction: nextDirection });
            }
          }
        } else {
          active.lastCursor = nextCursor;
        }
      } catch {
        // Keep the initial pointer vector when global sampling is unavailable.
      }
      if (pointer.current === pressed && pressed.dragging) {
        directionTimer = window.setTimeout(() => { void sampleDragDirection(); }, DRAG_DIRECTION_POLL_MS);
      }
    };
    void sampleDragDirection();
    void getCurrentWindow().startDragging().finally(() => {
      window.clearTimeout(directionTimer);
      if (!pointer.current?.dragging) return;
      pointer.current = null;
      dispatchBehavior({ type: 'dragEnded' });
      void api.persistCompanionPosition();
    });
  };

  const endPointer = (event: React.PointerEvent<HTMLElement>) => {
    const pressed = pointer.current;
    if (!pressed || pressed.id !== event.pointerId) return;
    pointer.current = null;
    if (event.currentTarget.hasPointerCapture(event.pointerId)) {
      event.currentTarget.releasePointerCapture(event.pointerId);
    }
    if (pressed.dragging) {
      dispatchBehavior({ type: 'dragEnded' });
      return;
    }
    const now = performance.now();
    clickTimes.current = [...clickTimes.current.filter(time => now - time <= CLICK_SEQUENCE_MS), now];
    dispatchBehavior({ type: 'clicked', clickCount: clickTimes.current.length });
  };

  const setInteractionMode = (mode: CompanionSettings['interactionMode']) => {
    setContextMenuOpen(false);
    setSettings(current => ({
      ...current,
      interactionMode: mode,
      lockPosition: mode === 'locked',
    }));
    void api.setCompanionInteraction(mode);
  };

  const label = settings.privacyMode
    ? projection?.terminal
      ? t('companion.taskFinished')
      : projection
        ? t('companion.working')
        : t('companion.ready')
    : projection?.label ?? t('companion.ready');

  return (
    <main
      className="companion-window-root"
      data-state={stableState}
      data-behavior={effectiveBehavior}
      data-facing={behavior === 'walkingLeft' || behavior === 'draggingLeft' ? 'left' : 'right'}
      data-visible={visible}
      data-auto-walk={settings.autoWalk}
      data-look-direction={lookDirection ?? 'none'}
      data-interaction-mode={settings.interactionMode}
      onPointerEnter={() => dispatchBehavior({ type: 'hoverStarted' })}
      onPointerLeave={() => {
        if (!pointer.current) dispatchBehavior({ type: 'hoverEnded' });
      }}
      onPointerDown={beginPointer}
      onPointerMove={movePointer}
      onPointerUp={endPointer}
      onPointerCancel={endPointer}
      onDoubleClick={() => { void api.executeCompanionCommand('settings'); }}
      onContextMenu={(event) => {
        event.preventDefault();
        setContextMenuOpen(true);
      }}
    >
      {settings.showBubbles && projection && (
        <div className="companion-bubble" role="status">{label}</div>
      )}
      <Sprite
        runtime={runtime}
        state={stableState}
        behavior={effectiveBehavior}
        settings={settings}
        reducedMotion={reducedMotion}
        active={motionActive}
        terminal={Boolean(stableProjection?.terminal) && !terminalAnimationConsumed}
        lookDirection={stableState === 'idle' ? lookDirection : null}
        onAnimationComplete={() => {
          if (stableProjection?.terminal) setTerminalAnimationConsumed(true);
          dispatchBehavior({ type: 'animationCompleted' });
        }}
      />
      <div className="companion-shadow" aria-hidden="true" />
      {contextMenuOpen && (
        <div className="companion-context-menu" role="menu" onPointerDown={event => event.stopPropagation()}>
          <button type="button" role="menuitem" onClick={() => { setContextMenuOpen(false); void api.executeCompanionCommand('settings'); }}>
            {t('companion.configure')}
          </button>
          {settings.interactionMode === 'locked' ? (
            <button type="button" role="menuitem" onClick={() => setInteractionMode('smart')}>{t('companion.unlockPosition')}</button>
          ) : (
            <button type="button" role="menuitem" onClick={() => setInteractionMode('locked')}>{t('companion.lockPosition')}</button>
          )}
          <button type="button" role="menuitem" onClick={() => setInteractionMode('click_through')}>{t('companion.interactionClickThrough')}</button>
        </div>
      )}
    </main>
  );
}
