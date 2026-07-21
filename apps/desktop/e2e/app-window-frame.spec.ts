import { expect, test } from '@playwright/test';

test.beforeEach(async ({ page }) => {
  await page.addInitScript(() => {
    localStorage.setItem('nexa-locale', 'en');
    localStorage.setItem('nexa-theme', 'dream');
    localStorage.setItem('last-health-check-at', String(Date.now()));
    localStorage.setItem('last-insights-at', String(Date.now()));

    const callbackMap = new Map<number, (event: unknown) => void>();
    const listeners = new Map<number, { event: string; handlerId: number }>();
    const windowCommands: string[] = [];
    let callbackSeq = 1;
    let listenerSeq = 1;
    let maximized = false;

    const invoke = async (cmd: string, args: Record<string, unknown> = {}) => {
      if (cmd.startsWith('plugin:window|')) windowCommands.push(cmd);
      switch (cmd) {
        case 'plugin:event|listen': {
          const listenerId = listenerSeq++;
          listeners.set(listenerId, {
            event: String(args.event ?? ''),
            handlerId: Number(args.handler ?? 0),
          });
          return listenerId;
        }
        case 'plugin:event|unlisten':
          listeners.delete(Number(args.eventId ?? 0));
          return null;
        case 'plugin:window|is_maximized':
          return maximized;
        case 'plugin:window|toggle_maximize':
          maximized = !maximized;
          return null;
        case 'get_wizard_state':
          return null;
        default:
          return null;
      }
    };

    (window as unknown as { __NEXA_WINDOW_COMMANDS__: string[] }).__NEXA_WINDOW_COMMANDS__ = windowCommands;
    (window as unknown as { __TAURI_INTERNALS__: unknown }).__TAURI_INTERNALS__ = {
      invoke,
      metadata: { currentWindow: { label: 'main' } },
      transformCallback: (callback: (event: unknown) => void) => {
        const id = callbackSeq++;
        callbackMap.set(id, callback);
        return id;
      },
      unregisterCallback: (id: number) => callbackMap.delete(id),
      convertFileSrc: (filePath: string) => filePath,
    };
    (window as unknown as { __TAURI_EVENT_PLUGIN_INTERNALS__: unknown }).__TAURI_EVENT_PLUGIN_INTERNALS__ = {
      unregisterListener: (_event: string, eventId: number) => listeners.delete(eventId),
    };
  });
});

test('uses a themed window frame with working native controls', async ({ page }) => {
  await page.goto('/missing-route');

  const titlebar = page.getByTestId('app-titlebar');
  const dragRegion = page.getByTestId('app-titlebar-drag-region');
  await expect(titlebar).toBeVisible();
  await expect(dragRegion).toHaveAttribute('data-tauri-drag-region', '');

  const dreamBackground = await titlebar.evaluate((element) => getComputedStyle(element).backgroundColor);
  await page.evaluate(() => {
    document.documentElement.classList.remove('theme-dream');
    document.documentElement.classList.add('theme-light');
  });
  await expect.poll(() => titlebar.evaluate((element) => getComputedStyle(element).backgroundColor))
    .not.toBe(dreamBackground);

  await page.getByRole('button', { name: 'Minimize window' }).click();
  await page.getByRole('button', { name: 'Maximize window' }).click();
  await expect(page.getByRole('button', { name: 'Restore window' })).toBeVisible();
  await page.getByRole('button', { name: 'Close window' }).click();

  const commands = await page.evaluate(() =>
    (window as unknown as { __NEXA_WINDOW_COMMANDS__: string[] }).__NEXA_WINDOW_COMMANDS__,
  );
  expect(commands).toEqual(expect.arrayContaining([
    'plugin:window|minimize',
    'plugin:window|toggle_maximize',
    'plugin:window|close',
  ]));
});
