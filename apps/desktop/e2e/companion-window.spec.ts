import { expect, test } from '@playwright/test';

test.beforeEach(async ({ page }) => {
  await page.addInitScript(() => {
    localStorage.setItem('nexa-locale', 'en');
    let callbackSeq = 1;
    let listenerSeq = 1;
    const callbacks = new Map<number, (event: unknown) => void>();
    const listeners = new Map<string, number>();
    const companionInvocations: string[] = [];
    const invoke = async (cmd: string, args?: { event?: string; handler?: number }) => {
      switch (cmd) {
        case 'plugin:event|listen':
          if (args?.event && typeof args.handler === 'number') listeners.set(args.event, args.handler);
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
              autoWalk: false,
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
        case 'scan_companion_packs_cmd': return { packs: [], errors: [] };
        case 'get_global_companion_projection_cmd':
          return { runId: 'run-1', state: 'runningTool', label: 'private task title', terminal: false };
        case 'companion_renderer_ready_cmd':
          (window as unknown as { __companionReady?: boolean }).__companionReady = true;
          return null;
        default:
          if (cmd === 'show_companion_cmd' || cmd === 'hide_companion_cmd') {
            companionInvocations.push(cmd);
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
      __emitTauri?: (event: string, payload: unknown) => void;
    }).__companionInvocations = companionInvocations;
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
  await page.goto('/companion');

  const companion = page.locator('.companion-window-root');
  await expect(companion).toHaveAttribute('data-state', 'runningTool');
  await expect(page.locator('.companion-fallback')).toBeVisible();
  await expect(companion).toHaveCSS('transform', 'matrix(2, 0, 0, 2, 0, 0)');
  await expect.poll(async () => companion.evaluate((element) => ({
    width: element.getBoundingClientRect().width,
    height: element.getBoundingClientRect().height,
  }))).toEqual({ width: 480, height: 520 });
  await expect(page.getByRole('status')).toHaveText('Nexa is working');
  await expect(page.getByText('private task title')).toHaveCount(0);
  await expect(page.locator('[data-testid="app-window-frame"]')).toHaveCount(0);
  await expect.poll(() => page.evaluate(() => (
    window as unknown as { __companionReady?: boolean }
  ).__companionReady)).toBe(true);
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
