import { expect, test } from '@playwright/test';

test.beforeEach(async ({ page }) => {
  await page.addInitScript(() => {
    localStorage.setItem('nexa-locale', 'en');
    let callbackSeq = 1;
    let listenerSeq = 1;
    const callbacks = new Map<number, (event: unknown) => void>();
    const listeners = new Map<string, number>();
    const companionInvocations: string[] = [];
    const positionWrites: unknown[] = [];
    const lifecycleMarks: Array<{ kind: string; at: number }> = [];
    let packVersion = 1;
    let assetReads = 0;
    const invoke = async (cmd: string, args: Record<string, unknown> = {}) => {
      if (cmd.startsWith('plugin:window|')) companionInvocations.push(cmd);
      switch (cmd) {
        case 'plugin:event|listen':
          if (typeof args.event === 'string' && typeof args.handler === 'number') {
            listeners.set(args.event, args.handler);
          }
          return listenerSeq++;
        case 'plugin:event|unlisten': return null;
        case 'get_app_config_cmd':
          return {
            companion: {
              enabled: true,
              selectedPetId: null,
              displayMode: 'always',
              interactionMode: 'smart',
              autoShowOnStart: true,
              continueWhenMainHidden: localStorage.getItem('nexa-test-continue-hidden') !== 'false',
              scale: 2,
              animationFpsCap: 24,
              reducedMotion: false,
              idleActions: true,
              autoWalk: localStorage.getItem('nexa-test-auto-walk') === 'true',
              showBubbles: true,
              bubbleTaskTitles: false,
              privacyMode: true,
              successHoldMs: 4000,
              failureHoldMs: 6000,
              alwaysOnTop: true,
              visibleOnAllWorkspaces: false,
              lockPosition: false,
              activeRunPolicy: 'highest_priority',
              pinnedRunId: null,
              pinnedProjectId: null,
              monitorId: null,
              anchor: 'bottom_right',
              position: null,
              edgeSnap: true,
              avoidTaskbar: true,
              allowMonitorMove: true,
              codexImportPath: null,
            },
          };
        case 'scan_companion_packs_cmd':
          return {
            packs: [{
              id: 'test-pet',
              displayName: 'Decoded Test Pet',
              description: null,
              dialect: 'nexa_v2',
              compatibility: 'native',
              spritesheetPath: 'pet.svg',
              contentHash: `pack-${packVersion}`,
              frame: { width: 1, height: 1, columns: 1, rows: 1 },
              animations: {
                idle: { frames: [0], fps: 12, looping: true, fallback: null },
                runningTool: { frames: [0], fps: 12, looping: true, fallback: 'idle' },
                clicked: { frames: [0], fps: 12, looping: false, fallback: 'idle' },
                beingPetted: { frames: [0], fps: 12, looping: false, fallback: 'idle' },
              },
              experimentalFeatures: [],
              managed: false,
            }],
            errors: [],
          };
        case 'read_companion_asset_cmd': {
          assetReads += 1;
          await new Promise(resolve => window.setTimeout(resolve, 80));
          lifecycleMarks.push({ kind: `decoded-source-${String(args.contentHash ?? '')}`, at: performance.now() });
          const color = String(args.contentHash ?? '').endsWith('2') ? '%23ec4899' : '%238b5cf6';
          return {
            dataUrl: `data:image/svg+xml,%3Csvg xmlns='http://www.w3.org/2000/svg' width='1' height='1'%3E%3Crect width='1' height='1' fill='${color}'/%3E%3C/svg%3E`,
            contentHash: String(args.contentHash ?? ''),
          };
        }
        case 'get_global_companion_projection_cmd':
          if (localStorage.getItem('nexa-test-companion-idle') === 'true') {
            return { runId: 'idle-run', state: 'idle', label: 'Ready', terminal: false };
          }
          return { runId: 'run-1', state: 'runningTool', label: 'private task title', terminal: false };
        case 'plugin:window|outer_position':
          return { x: 400, y: 500 };
        case 'plugin:window|outer_size':
          return { width: 288, height: 336 };
        case 'plugin:window|current_monitor':
          return {
            name: 'Test Monitor',
            scaleFactor: 1,
            position: { x: 0, y: 0 },
            size: { width: 800, height: 700 },
            workArea: {
              position: { x: 0, y: 0 },
              size: { width: 800, height: 660 },
            },
          };
        case 'plugin:window|set_position':
          positionWrites.push(args.value);
          return null;
        case 'companion_renderer_ready_cmd':
          (window as unknown as { __companionReady?: boolean }).__companionReady = true;
          lifecycleMarks.push({ kind: 'renderer-ready', at: performance.now() });
          return null;
        default:
          if (
            cmd === 'show_companion_cmd'
            || cmd === 'hide_companion_cmd'
            || cmd === 'set_companion_interaction_cmd'
          ) {
            companionInvocations.push(
              cmd === 'set_companion_interaction_cmd'
                ? `${cmd}:${String(args.mode ?? '')}`
                : cmd,
            );
          }
          return null;
      }
    };
    (window as unknown as { __TAURI_INTERNALS__: unknown }).__TAURI_INTERNALS__ = {
      invoke,
      metadata: {
        currentWindow: { label: 'companion' },
        currentWebview: { windowLabel: 'companion', label: 'companion' },
      },
      transformCallback: (callback: (event: unknown) => void) => {
        const id = callbackSeq++;
        callbacks.set(id, callback);
        return id;
      },
      unregisterCallback: (id: number) => callbacks.delete(id),
      convertFileSrc: (path: string) => path,
    };
    (window as unknown as { __TAURI_EVENT_PLUGIN_INTERNALS__: unknown }).__TAURI_EVENT_PLUGIN_INTERNALS__ = {
      unregisterListener: () => undefined,
    };
    (window as unknown as {
      __companionInvocations?: string[];
      __companionLifecycleMarks?: Array<{ kind: string; at: number }>;
      __companionAssetReads?: () => number;
      __companionPositionWrites?: unknown[];
      __setCompanionPackVersion?: (version: number) => void;
      __emitTauri?: (event: string, payload: unknown) => void;
    }).__companionInvocations = companionInvocations;
    (window as unknown as {
      __companionLifecycleMarks?: Array<{ kind: string; at: number }>;
      __companionAssetReads?: () => number;
      __setCompanionPackVersion?: (version: number) => void;
    }).__companionLifecycleMarks = lifecycleMarks;
    (window as unknown as { __companionAssetReads?: () => number }).__companionAssetReads = () => assetReads;
    (window as unknown as { __companionPositionWrites?: unknown[] }).__companionPositionWrites = positionWrites;
    (window as unknown as { __setCompanionPackVersion?: (version: number) => void }).__setCompanionPackVersion = version => {
      packVersion = version;
    };
    (window as unknown as {
      __emitTauri?: (event: string, payload: unknown) => void;
    }).__emitTauri = (event, payload) => {
      const callbackId = listeners.get(event);
      if (callbackId === undefined) return;
      callbacks.get(callbackId)?.({ event, payload });
    };
  });
});

test('companion route is independent, task-aware, and privacy-safe', async ({ page }) => {
  await page.setViewportSize({ width: 288, height: 336 });
  await page.goto('/companion');

  const companion = page.locator('.companion-window-root');
  await expect(companion).toHaveAttribute('data-state', 'runningTool');
  await expect(page.locator('.companion-sprite')).toBeVisible();
  await expect(page.locator('.companion-fallback')).toHaveCount(0);
  await expect(companion).toHaveCSS('transform', 'none');
  await expect.poll(async () => companion.evaluate((element) => ({
    width: element.getBoundingClientRect().width,
    height: element.getBoundingClientRect().height,
  }))).toEqual({ width: 288, height: 336 });
  await expect(page.getByRole('status')).toHaveText('Nexa is working');
  await expect(page.getByText('private task title')).toHaveCount(0);
  await expect(page.locator('[data-testid="app-window-frame"]')).toHaveCount(0);
  await expect.poll(() => page.evaluate(() => (
    window as unknown as { __companionReady?: boolean }
  ).__companionReady)).toBe(true);
  const lifecycleMarks = await page.evaluate(() => (
    window as unknown as { __companionLifecycleMarks?: Array<{ kind: string; at: number }> }
  ).__companionLifecycleMarks ?? []);
  expect(lifecycleMarks.find(mark => mark.kind === 'renderer-ready')?.at).toBeGreaterThanOrEqual(
    lifecycleMarks.find(mark => mark.kind === 'decoded-source-pack-1')?.at ?? Number.POSITIVE_INFINITY,
  );
});

test('decoded pack refresh is atomic and pointer behaviors are reachable', async ({ page }) => {
  await page.setViewportSize({ width: 144, height: 168 });
  await page.goto('/companion');
  const companion = page.locator('.companion-window-root');
  const sprite = page.locator('.companion-sprite');
  await expect(sprite).toBeVisible();

  await page.evaluate(() => {
    const testWindow = window as unknown as {
      __setCompanionPackVersion?: (version: number) => void;
      __emitTauri?: (event: string, payload: unknown) => void;
    };
    testWindow.__setCompanionPackVersion?.(2);
    testWindow.__emitTauri?.('companion://settings-changed', null);
  });
  await page.waitForTimeout(30);
  await expect(sprite).toBeVisible();
  await expect(page.locator('.companion-fallback')).toHaveCount(0);
  await expect.poll(() => page.evaluate(() => (
    window as unknown as { __companionAssetReads?: () => number }
  ).__companionAssetReads?.())).toBe(2);

  await companion.hover();
  await expect(companion).toHaveAttribute('data-behavior', 'hovering');
  await companion.click();
  await expect(companion).toHaveAttribute('data-behavior', 'clicked');
  await companion.click({ clickCount: 3, delay: 30 });
  await expect(companion).toHaveAttribute('data-behavior', 'beingPetted');

  await companion.click({ button: 'right' });
  const menu = page.getByRole('menu');
  await expect(menu).toBeVisible();
  await menu.getByRole('menuitem', { name: 'Lock position' }).click();
  await expect(companion).toHaveAttribute('data-interaction-mode', 'locked');
  await expect.poll(() => page.evaluate(() => (
    window as unknown as { __companionInvocations?: string[] }
  ).__companionInvocations?.at(-1))).toBe('set_companion_interaction_cmd:locked');
});

test('continue-when-hidden gates automatic companion visibility', async ({ page }) => {
  await page.addInitScript(() => localStorage.setItem('nexa-test-continue-hidden', 'false'));
  await page.goto('/companion');
  await expect.poll(() => page.evaluate(() => (
    window as unknown as { __companionReady?: boolean }
  ).__companionReady)).toBe(true);

  await page.evaluate(() => (
    window as unknown as { __emitTauri?: (event: string, payload: unknown) => void }
  ).__emitTauri?.('companion://main-visibility', false));
  await expect.poll(() => page.evaluate(() => {
    const calls = (window as unknown as { __companionInvocations?: string[] }).__companionInvocations ?? [];
    return calls.at(-1);
  })).toBe('hide_companion_cmd');

  await page.evaluate(() => (
    window as unknown as { __emitTauri?: (event: string, payload: unknown) => void }
  ).__emitTauri?.('companion://main-visibility', true));
  await expect.poll(() => page.evaluate(() => {
    const calls = (window as unknown as { __companionInvocations?: string[] }).__companionInvocations ?? [];
    return calls.at(-1);
  })).toBe('show_companion_cmd');
});

test('auto-walk schedules bounded native position updates while idle', async ({ page }) => {
  await page.setViewportSize({ width: 288, height: 336 });
  await page.goto('/companion');
  await page.evaluate(() => {
    localStorage.setItem('nexa-test-auto-walk', 'true');
    localStorage.setItem('nexa-test-companion-idle', 'true');
  });
  await page.reload();
  await page.mouse.move(500, 500);
  await expect.poll(() => page.evaluate(() => (
    window as unknown as { __companionReady?: boolean }
  ).__companionReady)).toBe(true);
  await page.evaluate(() => {
    (window as unknown as { __emitTauri?: (event: string, payload: unknown) => void })
      .__emitTauri?.('companion://visibility', true);
  });

  const companion = page.locator('.companion-window-root');
  await expect(companion).toHaveAttribute('data-state', 'idle');
  await expect(companion).toHaveAttribute('data-behavior', 'idle');
  await expect(companion).toHaveAttribute('data-visible', 'true');
  await expect(companion).toHaveAttribute('data-auto-walk', 'true');
  await expect.poll(() => page.evaluate(() => (
    (window as unknown as { __companionInvocations?: string[] })
      .__companionInvocations?.includes('plugin:window|outer_position') ?? false
  )), { timeout: 12_000 }).toBe(true);
  await expect.poll(() => page.evaluate(() => (
    (window as unknown as { __companionPositionWrites?: unknown[] })
      .__companionPositionWrites?.length ?? 0
  )), { timeout: 13_000 }).toBeGreaterThan(0);
  await expect(companion).toHaveAttribute('data-behavior', /walkingLeft|walkingRight/);
  const lastX = await page.evaluate(() => {
    const writes = (window as unknown as { __companionPositionWrites?: unknown[] })
      .__companionPositionWrites ?? [];
    const value = writes.at(-1) as {
      Physical?: { x?: number };
      position?: { x?: number };
      x?: number;
    } | undefined;
    return value?.Physical?.x ?? value?.position?.x ?? value?.x ?? null;
  });
  expect(lastX).not.toBeNull();
  expect(lastX!).toBeGreaterThanOrEqual(12);
  expect(lastX!).toBeLessThanOrEqual(500);
});
