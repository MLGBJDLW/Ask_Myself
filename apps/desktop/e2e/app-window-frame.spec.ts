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
    let releaseWizardState: (() => void) | null = null;

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
        case 'get_wizard_state_cmd':
          if (localStorage.getItem('nexa-test-delay-wizard') === 'true') {
            await new Promise<void>(resolve => { releaseWizardState = resolve; });
          }
          return null;
        default:
          return null;
      }
    };

    (window as unknown as { __NEXA_WINDOW_COMMANDS__: string[] }).__NEXA_WINDOW_COMMANDS__ = windowCommands;
    (window as unknown as { __releaseWizardState?: () => void }).__releaseWizardState = () => {
      releaseWizardState?.();
      releaseWizardState = null;
    };
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

  const dreamBackground = await titlebar.evaluate((element) => getComputedStyle(element).backgroundImage);
  await page.evaluate(() => {
    document.documentElement.classList.remove('theme-dream');
    document.documentElement.classList.add('theme-light');
  });
  await expect.poll(() => titlebar.evaluate((element) => getComputedStyle(element).backgroundImage))
    .not.toBe(dreamBackground);

  const regularHeight = (await titlebar.boundingBox())!.height;
  await page.evaluate(() => document.documentElement.style.setProperty('--theme-titlebar-height', '1.8rem'));
  await expect.poll(() => titlebar.boundingBox().then(box => box?.height ?? 0)).toBeLessThan(regularHeight);
  await page.evaluate(() => { document.documentElement.dataset.cursorStyle = 'precise'; });
  await expect(page.getByRole('button', { name: 'Minimize window' })).toHaveCSS('cursor', 'crosshair');
  await page.evaluate(() => {
    document.documentElement.style.setProperty('--theme-base-font-size', '18px');
    const sample = document.createElement('div');
    sample.dataset.testid = 'theme-rem-sample';
    sample.className = 'text-sm';
    document.body.appendChild(sample);
  });
  await expect.poll(() => page.evaluate(() => getComputedStyle(document.documentElement).fontSize)).toBe('18px');
  await expect.poll(() => page.getByTestId('theme-rem-sample').evaluate(element => Number.parseFloat(getComputedStyle(element).fontSize))).toBeGreaterThan(14);

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

test('keeps the navigation rail fixed, optically centered, and metadata visible', async ({ page }) => {
  await page.goto('/missing-route');

  const rail = page.getByTestId('app-navigation-rail');
  await expect(rail).toBeVisible();
  await expect(rail).toHaveCSS('width', '56px');
  await expect(page.getByTestId('app-version')).toBeVisible();

  const axes = await Promise.all([
    rail.boundingBox(),
    page.getByRole('link', { name: 'Nexa' }).boundingBox(),
    page.getByRole('link', { name: 'Search' }).boundingBox(),
    page.getByTestId('app-version').boundingBox(),
  ]);
  const [railBox, logoBox, searchBox, versionBox] = axes;
  expect(railBox && logoBox && searchBox && versionBox).toBeTruthy();
  const railAxis = railBox!.x + railBox!.width / 2;
  for (const box of [logoBox!, searchBox!, versionBox!]) {
    expect(Math.abs(box.x + box.width / 2 - railAxis)).toBeLessThanOrEqual(1);
  }
  expect(versionBox!.y + versionBox!.height).toBeLessThanOrEqual(railBox!.y + railBox!.height);
});

test('keeps native controls synchronized with the active color scheme', async ({ page }) => {
  await page.goto('/missing-route');

  await expect.poll(() => page.evaluate(() => document.documentElement.style.colorScheme))
    .toBe('dark');

  await page.evaluate(() => localStorage.setItem('nexa-active-theme-v1', 'light'));
  await page.reload();
  await expect.poll(() => page.evaluate(() => document.documentElement.style.colorScheme))
    .toBe('light');

  await page.evaluate(() => localStorage.setItem('nexa-active-theme-v1', 'midnight'));
  await page.reload();
  await expect.poll(() => page.evaluate(() => document.documentElement.style.colorScheme))
    .toBe('dark');
});

test('reveals the native window onto a branded startup surface', async ({ page }) => {
  await page.goto('/missing-route');
  await page.evaluate(() => localStorage.setItem('nexa-test-delay-wizard', 'true'));
  await page.reload({ waitUntil: 'domcontentloaded' });

  const splash = page.getByTestId('startup-splash');
  await expect(splash).toBeVisible();
  await expect(splash.locator('.startup-splash__logo')).toBeVisible();
  await expect.poll(() => page.evaluate(() =>
    (window as unknown as { __NEXA_WINDOW_COMMANDS__: string[] }).__NEXA_WINDOW_COMMANDS__,
  )).toContain('plugin:window|show');
  await expect.poll(() => page.evaluate(() =>
    (window as unknown as { __NEXA_WINDOW_COMMANDS__: string[] }).__NEXA_WINDOW_COMMANDS__,
  )).toContain('plugin:window|set_focus');
  await page.evaluate(() => (
    window as unknown as { __releaseWizardState?: () => void }
  ).__releaseWizardState?.());
  await expect(splash).toHaveCount(0);
  await expect(page.getByText('404')).toBeVisible();
});

test('reveals the static startup surface while the React bootstrap is still loading', async ({ page }) => {
  let releaseBootstrap!: () => void;
  const gate = new Promise<void>(resolve => { releaseBootstrap = resolve; });
  const requested = page.waitForRequest('**/src/bootstrap.tsx*');
  await page.route('**/src/bootstrap.tsx*', async route => {
    await gate;
    await route.continue();
  });
  await page.goto('/missing-route', { waitUntil: 'domcontentloaded' });
  await requested;
  try {
    await expect(page.getByTestId('startup-splash')).toBeVisible();
    await expect(page.getByText('404', { exact: true })).toHaveCount(0);
    await expect.poll(() => page.evaluate(() =>
      (window as unknown as { __NEXA_WINDOW_COMMANDS__: string[] }).__NEXA_WINDOW_COMMANDS__,
    )).toContain('plugin:window|show');
  } finally {
    releaseBootstrap();
  }
  await expect(page.getByText('404', { exact: true })).toBeVisible();
});

test('offers a working reload when the interface bootstrap cannot load', async ({ page }) => {
  await page.route('**/src/bootstrap.tsx*', route => route.abort());
  await page.goto('/missing-route');
  await expect(page.getByRole('alert')).toHaveText('Unable to load Nexa.');
  await page.unroute('**/src/bootstrap.tsx*');
  await page.getByRole('button', { name: 'Reload', exact: true }).click();
  await expect(page.getByText('404', { exact: true })).toBeVisible();
});

test('keeps route module failures behind the recoverable application error screen', async ({ page }) => {
  await page.route('**/src/pages/ChatPage.tsx*', route => route.abort());
  await page.goto('/chat/route-load-probe');
  await expect(page.getByRole('heading', { name: 'Something went wrong' })).toBeVisible();
  await expect(page.getByText('Unexpected Application Error!')).toHaveCount(0);
  await page.unroute('**/src/pages/ChatPage.tsx*');
  await page.getByRole('button', { name: 'Restart', exact: true }).click();
  await expect(page.getByTestId('chat-input-textarea')).toBeVisible();
});

test('hydrates the startup surface from the last validated theme snapshot before React', async ({ page }) => {
  await page.addInitScript(() => {
    localStorage.setItem('nexa-startup-appearance-v1', JSON.stringify({
      version: 1,
      mode: 'light',
      canvas: '#fef3c7',
      panel: '#fffbeb',
      text: '#422006',
      accent: '#d97706',
      muted: '#92400e',
      tagline: 'Make the next action clear.',
    }));
  });
  await page.route('**/src/main.tsx*', route => route.abort());
  await page.goto('/missing-route', { waitUntil: 'domcontentloaded' });

  const splash = page.getByTestId('startup-splash');
  await expect(splash).toBeVisible();
  await expect(splash.locator('.startup-splash__tagline')).toHaveText('Make the next action clear.');
  await expect.poll(() => page.evaluate(() => {
    const style = getComputedStyle(document.documentElement);
    return {
      canvas: style.getPropertyValue('--startup-canvas').trim(),
      accent: style.getPropertyValue('--startup-accent').trim(),
      scheme: document.documentElement.style.colorScheme,
    };
  })).toEqual({ canvas: '#fef3c7', accent: '#d97706', scheme: 'light' });

});
