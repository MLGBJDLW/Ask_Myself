import { readFileSync } from 'node:fs';
import { join } from 'node:path';
import { expect, test } from '@playwright/test';

const source = readFileSync(join(process.cwd(), 'src-tauri', 'src', 'browser', 'scripts.rs'), 'utf8');
const runtimeSource = source.match(/pub const BROWSER_INIT_SCRIPT: &str = r#"\r?\n([\s\S]*?)\r?\n"#;/)?.[1]
  .replace('__NEXA_PICK_TOKEN__', JSON.stringify('playwright-picker-token'));
const takeoverSource = source.match(/pub fn browser_takeover_script[\s\S]*?\br#"\r?\n([\s\S]*?)\r?\n"#\r?\n\s*\.replace/)?.[1]
  .replace('__NEXA_TAKEOVER_URL__', JSON.stringify('nexa-user-input://playwright-takeover-token'))
  .replace('__NEXA_TAKEOVER_TOKEN__', JSON.stringify('playwright-takeover-token'));

if (!runtimeSource) throw new Error('Could not extract the native Browser Workspace interaction runtime');
if (!takeoverSource) throw new Error('Could not extract the native Browser Workspace takeover guard');

test('Agent browser interaction shows cursor motion and commits verified pointer actions', async ({ page }) => {
  await page.setContent(`
    <!doctype html>
    <button id="source">Open details</button>
    <button id="destination">Destination</button>
    <script>
      window.actionEvents = [];
      for (const type of ['pointerover', 'pointermove', 'pointerdown', 'mousedown', 'pointerup', 'mouseup', 'click', 'dblclick', 'dragstart', 'dragenter', 'dragover', 'drop', 'dragend']) {
        document.addEventListener(type, event => window.actionEvents.push({ type, target: event.target.id, buttons: event.buttons }), true);
      }
    </script>
  `);
  await page.addScriptTag({ content: runtimeSource });

  const observation = await observe(page);
  const sourceRef = observation.elements.find(element => element.name === 'Open details')?.ref;
  expect(sourceRef).toBeTruthy();
  const input = actionInput(observation, 'click', sourceRef!);
  const preview = await page.evaluate(value => (
    window as unknown as { __NEXA_BROWSER_RUNTIME__: BrowserBridge }
  ).__NEXA_BROWSER_RUNTIME__.previewAction(value), input);

  expect(preview.durationMs).toBeGreaterThanOrEqual(180);
  await expect(page.locator('[data-nexa-agent-cursor]')).toBeVisible();
  await page.waitForTimeout(preview.durationMs + 20);
  await expect(page.locator('[data-nexa-agent-cursor]')).toHaveCSS('pointer-events', 'none');
  await page.evaluate(value => (
    window as unknown as { __NEXA_BROWSER_RUNTIME__: BrowserBridge }
  ).__NEXA_BROWSER_RUNTIME__.act(value), input);

  const clickEvents = await page.evaluate(() => (
    window as unknown as { actionEvents: Array<{ type: string; target: string }> }
  ).actionEvents.filter(event => event.target === 'source').map(event => event.type));
  expect(clickEvents).toEqual(expect.arrayContaining([
    'pointerover', 'pointermove', 'pointerdown', 'mousedown', 'pointerup', 'mouseup', 'click',
  ]));
  const buttonStates = await page.evaluate(() => (
    window as unknown as { actionEvents: Array<{ type: string; buttons: number }> }
  ).actionEvents.filter(event => ['pointermove', 'pointerdown', 'pointerup'].includes(event.type)));
  expect(buttonStates.find(event => event.type === 'pointermove')?.buttons).toBe(0);
  expect(buttonStates.find(event => event.type === 'pointerdown')?.buttons).toBe(1);
  expect(buttonStates.find(event => event.type === 'pointerup')?.buttons).toBe(0);
  await expect(page.locator('[data-nexa-agent-click]')).toBeVisible();

  const next = await observe(page);
  const nextSourceRef = next.elements.find(element => element.name === 'Open details')!.ref;
  await page.evaluate(value => (
    window as unknown as { __NEXA_BROWSER_RUNTIME__: BrowserBridge }
  ).__NEXA_BROWSER_RUNTIME__.act(value), actionInput(next, 'double_click', nextSourceRef));
  expect(await page.evaluate(() => (
    window as unknown as { actionEvents: Array<{ type: string }> }
  ).actionEvents.filter(event => event.type === 'dblclick').length)).toBe(1);
});

test('screenshot confirmation preserves the element references advertised to the agent', async ({ page }) => {
  await page.setContent('<!doctype html><button onclick="this.textContent=\'Verified\'">Open details</button>');
  await page.addScriptTag({ content: runtimeSource });
  const advertised = await observe(page);
  const ref = advertised.elements.find(element => element.name === 'Open details')!.ref;
  await page.screenshot();
  // This is the native observation path: DOM snapshot, screenshot, then a
  // second snapshot that verifies the captured page before returning refs.
  await observe(page);
  await page.evaluate(value => (
    window as unknown as { __NEXA_BROWSER_RUNTIME__: BrowserBridge }
  ).__NEXA_BROWSER_RUNTIME__.act(value), actionInput(advertised, 'click', ref));
  expect((await observe(page)).elements.some(element => element.name === 'Verified')).toBe(true);
});

test('an unrelated live counter does not invalidate an unchanged action target', async ({ page }) => {
  await page.setContent('<!doctype html><p id="fps">FPS 60</p><button onclick="this.textContent=\'Paused\'">Pause</button>');
  await page.addScriptTag({ content: runtimeSource });
  const advertised = await observe(page);
  const ref = advertised.elements.find(element => element.name === 'Pause')!.ref;
  await page.locator('#fps').evaluate(el => { el.textContent = 'FPS 59'; });
  await page.evaluate(value => (
    window as unknown as { __NEXA_BROWSER_RUNTIME__: BrowserBridge }
  ).__NEXA_BROWSER_RUNTIME__.act(value), actionInput(advertised, 'click', ref));
  expect((await observe(page)).elements.some(element => element.name === 'Paused')).toBe(true);
});

for (const owner of ['row', 'card', 'inline']) {
test(`a reused action control cannot act on changed ${owner} context`, async ({ page }) => {
  const contents = '<span id="account" style="display:inline-block;width:120px">Account A</span><span><button aria-label="Delete" onclick="window.deleted=document.getElementById(\'account\').textContent">×</button></span>';
  await page.setContent(`<!doctype html><p id="fps">FPS 60</p>${owner === 'inline' ? contents : `<div ${owner === 'row' ? 'role="row"' : 'class="card"'}>${contents}</div>`}`);
  await page.addScriptTag({ content: runtimeSource });
  const advertised = await observe(page);
  const ref = advertised.elements.find(element => element.name === 'Delete')!.ref;
  await page.locator('#account').evaluate(el => { el.textContent = 'Account B'; });
  // The element, accessible button name and geometry are all unchanged.
  const refreshed = await observe(page);
  expect(refreshed.elements.find(element => element.ref === ref)?.bounds)
    .toEqual(advertised.elements.find(element => element.ref === ref)?.bounds);
  await expect(page.evaluate(value => (
    window as unknown as { __NEXA_BROWSER_RUNTIME__: BrowserBridge }
  ).__NEXA_BROWSER_RUNTIME__.act(value), actionInput(advertised, 'click', ref))).rejects.toThrow(/stale observation/);
  expect(await page.evaluate(() => (window as Window & { deleted?: string }).deleted)).toBeUndefined();
  await page.locator('#fps').evaluate(el => { el.textContent = 'FPS 58'; });
  await page.evaluate(value => (
    window as unknown as { __NEXA_BROWSER_RUNTIME__: BrowserBridge }
  ).__NEXA_BROWSER_RUNTIME__.act(value), actionInput(refreshed, 'click', ref));
  expect(await page.evaluate(() => (window as Window & { deleted?: string }).deleted)).toBe('Account B');
});
}

test('trusted clicks reject a row recycled between preparation, arming and commit', async ({ page }) => {
  await page.setContent('<!doctype html><p id="fps">FPS 60</p><div role="row"><a href="#account-a" id="account" style="display:inline-block;width:120px">Account A</a><button id="delete" aria-label="Delete" onclick="window.deleted=true">×</button></div>');
  await page.addScriptTag({ content: runtimeSource });
  await page.addScriptTag({ content: takeoverSource });
  const binding = await targetBinding(page, '#delete');
  const bounds = (await page.locator('#delete').boundingBox())!;
  const expected = { kind: 'pointer' as const, x: bounds.x + bounds.width / 2, y: bounds.y + bounds.height / 2, button: 'left' as const, ...binding };
  const arm = (operationId: string) => page.evaluate(({ expected, operationId }) => (
    window as unknown as { __NEXA_TRUSTED_INPUT_GUARD__: TrustedInputGuard }
  ).__NEXA_TRUSTED_INPUT_GUARD__.arm('playwright-takeover-token', operationId, { pointerDown: 1, keyDown: 0, input: 0 }, expected), { expected, operationId });
  await page.locator('#account').evaluate(el => { el.textContent = 'Account B'; });
  expect(await arm('stale-preparation')).toBe(false);
  await page.locator('#account').evaluate(el => { el.textContent = 'Account A'; });
  expect(await arm('changed-after-arm')).toBe(true);
  await page.locator('#fps').evaluate(el => { el.textContent = 'FPS 59'; });
  await page.mouse.move(expected.x, expected.y);
  await page.mouse.down();
  await page.locator('#account').evaluate(el => { el.textContent = 'Account B'; });
  await page.mouse.up();
  expect(await page.evaluate(() => (window as Window & { deleted?: boolean }).deleted)).toBeUndefined();
  expect(await page.evaluate(() => (window as unknown as {
    __NEXA_TRUSTED_INPUT_GUARD__: { disarm(token: string, id: string): boolean }
  }).__NEXA_TRUSTED_INPUT_GUARD__.disarm('playwright-takeover-token', 'changed-after-arm'))).toBe(false);
});

for (const key of ['Space', 'Enter']) {
test(`trusted ${key} activation cannot commit against recycled row context`, async ({ page }) => {
  await page.setContent('<!doctype html><div role="row"><a href="#account-a" id="account">Account A</a><button id="delete" onclick="window.deleted=true" onkeydown="document.getElementById(\'account\').textContent=\'Account B\'">Delete</button></div>');
  await page.addScriptTag({ content: runtimeSource });
  await page.addScriptTag({ content: takeoverSource });
  await page.locator('#delete').focus();
  const binding = await targetBinding(page, '#delete');
  expect(await page.evaluate(({ binding, key }) => (
    window as unknown as { __NEXA_TRUSTED_INPUT_GUARD__: TrustedInputGuard }
  ).__NEXA_TRUSTED_INPUT_GUARD__.arm('playwright-takeover-token', 'key-recycle', { pointerDown: 0, keyDown: 1, input: 0 }, { kind: 'key', key: key === 'Space' ? ' ' : key, ...binding }), { binding, key })).toBe(true);
  await page.keyboard.press(key);
  expect(await page.evaluate(() => (window as Window & { deleted?: boolean }).deleted)).toBeUndefined();
});
}

test('trusted Enter input keeps the target editor value outside its context fence', async ({ page }) => {
  await page.setContent('<!doctype html><div role="row"><span>Account A</span><textarea id="editor">Text</textarea></div>');
  await page.addScriptTag({ content: runtimeSource });
  await page.addScriptTag({ content: takeoverSource });
  await page.locator('#editor').focus();
  const binding = await targetBinding(page, '#editor');
  expect(await page.evaluate(binding => (
    window as unknown as { __NEXA_TRUSTED_INPUT_GUARD__: TrustedInputGuard }
  ).__NEXA_TRUSTED_INPUT_GUARD__.arm('playwright-takeover-token', 'editor-enter', { pointerDown: 0, keyDown: 1, input: 1 }, { kind: 'key', key: 'Enter', ...binding }), binding)).toBe(true);
  await page.keyboard.press('Enter');
  await expect(page.locator('#editor')).toHaveValue('\nText');
  expect(await page.evaluate(() => (window as unknown as {
    __NEXA_TRUSTED_INPUT_GUARD__: { disarm(token: string, id: string): boolean }
  }).__NEXA_TRUSTED_INPUT_GUARD__.disarm('playwright-takeover-token', 'editor-enter'))).toBe(true);
});

test('Agent drag uses observation-scoped endpoints and takeover removes its visual cursor', async ({ page }) => {
  await page.setContent(`
    <!doctype html>
    <button id="source" draggable="true">Move card</button>
    <div id="destination" class="drop-zone">Drop zone</div>
    <script>
      window.actionEvents = [];
      for (const type of ['pointermove', 'mousemove', 'dragstart', 'dragenter', 'dragover', 'drop', 'dragend']) {
        document.addEventListener(type, event => { window.actionEvents.push({ type, target: event.target.id, buttons: event.buttons }); event.preventDefault(); }, true);
      }
    </script>
  `);
  await page.addScriptTag({ content: runtimeSource });

  const observation = await observe(page);
  const sourceRef = observation.elements.find(element => element.name === 'Move card')!.ref;
  const endRef = observation.elements.find(element => element.name === 'Drop zone')!.ref;
  const input = actionInput(observation, 'drag', sourceRef, endRef);
  const preview = await page.evaluate(value => (
    window as unknown as { __NEXA_BROWSER_RUNTIME__: BrowserBridge }
  ).__NEXA_BROWSER_RUNTIME__.previewAction(value), input);
  await page.waitForTimeout(preview.durationMs + 20);
  await page.evaluate(value => (
    window as unknown as { __NEXA_BROWSER_RUNTIME__: BrowserBridge }
  ).__NEXA_BROWSER_RUNTIME__.act(value), input);

  const dragEvents = await page.evaluate(() => (
    window as unknown as { actionEvents: Array<{ type: string; buttons: number }> }
  ).actionEvents);
  expect(dragEvents.filter(event => event.type.startsWith('drag') || event.type === 'drop').map(event => event.type)).toEqual([
    'dragstart', 'dragenter', 'dragover', 'drop', 'dragend',
  ]);
  const pointerMoves = dragEvents.filter(event => event.type === 'pointermove' && event.buttons === 1);
  expect(pointerMoves).toHaveLength(8);
  expect(dragEvents.some(event => event.type === 'pointermove' && event.buttons === 0)).toBe(true);
  await page.evaluate(() => (
    window as unknown as { __NEXA_BROWSER_RUNTIME__: BrowserBridge }
  ).__NEXA_BROWSER_RUNTIME__.invalidateForUserTakeover());
  await expect(page.locator('[data-nexa-agent-cursor]')).toHaveCount(0);
});

test('Agent pointer preparation scrolls targets into view and rejects covered elements', async ({ page }) => {
  await page.setViewportSize({ width: 800, height: 600 });
  await page.setContent('<!doctype html><button id="target" style="margin-top:1800px">Far target</button>');
  await page.addScriptTag({ content: runtimeSource });

  const observation = await observe(page);
  const targetRef = observation.elements.find(element => element.name === 'Far target')!.ref;
  const prepared = await page.evaluate(value => (
    window as unknown as { __NEXA_BROWSER_RUNTIME__: BrowserBridge }
  ).__NEXA_BROWSER_RUNTIME__.prepareNativePointer(value), actionInput(observation, 'hover', targetRef));
  expect(await page.evaluate(() => scrollY)).toBeGreaterThan(0);
  expect(prepared.bounds.y).toBeGreaterThanOrEqual(0);
  expect(prepared.bounds.y + prepared.bounds.height).toBeLessThanOrEqual(600);

  await page.setContent(`
    <!doctype html>
    <button id="covered">Covered target</button>
    <div style="position:fixed;inset:0;z-index:10;background:white">Overlay</div>
  `);
  await page.addScriptTag({ content: runtimeSource });
  const coveredObservation = await observe(page);
  const coveredRef = coveredObservation.elements.find(element => element.name === 'Covered target')!.ref;
  await expect(page.evaluate(value => (
    window as unknown as { __NEXA_BROWSER_RUNTIME__: BrowserBridge }
  ).__NEXA_BROWSER_RUNTIME__.prepareNativePointer(value), actionInput(coveredObservation, 'hover', coveredRef)))
    .rejects.toThrow(/covered by another element/);
});

test('pointer preparation establishes the post-scroll baseline for noop effect verification', async ({ page }) => {
  await page.setViewportSize({ width: 800, height: 600 });
  await page.setContent('<!doctype html><button id="target" style="margin-top:1800px">Far target</button>');
  await page.addScriptTag({ content: runtimeSource });

  const beforePreparation = await observe(page);
  const targetRef = beforePreparation.elements.find(element => element.name === 'Far target')!.ref;
  const prepared = await page.evaluate(value => (
    window as unknown as { __NEXA_BROWSER_RUNTIME__: BrowserBridge }
  ).__NEXA_BROWSER_RUNTIME__.prepareNativePointer(value), actionInput(beforePreparation, 'hover', targetRef));
  const afterPreparation = await observe(page);

  expect(afterPreparation.domFingerprint).not.toBe(beforePreparation.domFingerprint);
  expect(prepared.verificationBaseline).toEqual({
    url: afterPreparation.url,
    domFingerprint: afterPreparation.domFingerprint,
    userEpoch: afterPreparation.userEpoch,
  });

  const bounds = await page.locator('#target').boundingBox();
  if (!bounds) throw new Error('Expected prepared browser target bounds');
  await page.mouse.move(bounds.x + bounds.width / 2, bounds.y + bounds.height / 2);
  await page.locator('#target').evaluate(element => (element as HTMLButtonElement).click());
  const afterNoopActions = await observe(page);

  expect(snapshotChanged(prepared.verificationBaseline, afterNoopActions)).toBe(false);
});

test('same-length interactive attribute changes invalidate an observation', async ({ page }) => {
  await page.setContent('<!doctype html><a id="target" href="https://example.com/a">Open details</a>');
  await page.addScriptTag({ content: runtimeSource });
  const observation = await observe(page);
  const targetRef = observation.elements.find(element => element.name === 'Open details')!.ref;

  await page.locator('#target').evaluate((element) => {
    element.setAttribute('href', 'https://example.com/b');
  });

  await expect(page.evaluate(value => (
    window as unknown as { __NEXA_BROWSER_RUNTIME__: BrowserBridge }
  ).__NEXA_BROWSER_RUNTIME__.previewAction(value), actionInput(observation, 'click', targetRef)))
    .rejects.toThrow(/interactive page state changed/);
});

test('trusted input preparation validates, focuses, and selects editable targets', async ({ page }) => {
  await page.setContent('<!doctype html><input id="target" value="replace me"><button id="other">Other</button>');
  await page.addScriptTag({ content: runtimeSource });
  const observation = await observe(page);
  const targetRef = observation.elements.find(element => element.name === '')!.ref;
  const input = actionInput(observation, 'type', targetRef);

  const prepared = await page.evaluate(value => (
    window as unknown as { __NEXA_BROWSER_RUNTIME__: BrowserBridge }
  ).__NEXA_BROWSER_RUNTIME__.prepareTrustedText(value), input);
  expect(prepared.focused).toBe(true);
  expect(await page.locator('#target').evaluate((element) => (
    element as HTMLInputElement
  ).selectionStart)).toBe(0);
  expect(await page.locator('#target').evaluate((element) => (
    element as HTMLInputElement
  ).selectionEnd)).toBe('replace me'.length);

  const next = await observe(page);
  const otherRef = next.elements.find(element => element.name === 'Other')!.ref;
  await expect(page.evaluate(value => (
    window as unknown as { __NEXA_BROWSER_RUNTIME__: BrowserBridge }
  ).__NEXA_BROWSER_RUNTIME__.prepareTrustedText(value), actionInput(next, 'type', otherRef)))
    .rejects.toThrow(/editable target/);
});

test('trusted key preparation verifies focus in a same-origin iframe document', async ({ page }) => {
  await page.setContent(`
    <!doctype html>
    <iframe id="frame" srcdoc="<!doctype html><button id='target'>Frame action</button>"></iframe>
  `);
  await expect(page.frameLocator('#frame').locator('#target')).toBeVisible();
  await page.addScriptTag({ content: runtimeSource });
  const observation = await observe(page);
  const targetRef = observation.elements.find(element => element.name === 'Frame action')?.ref;
  expect(targetRef).toBeTruthy();

  const prepared = await page.evaluate(value => (
    window as unknown as { __NEXA_BROWSER_RUNTIME__: BrowserBridge }
  ).__NEXA_BROWSER_RUNTIME__.prepareTrustedKey(value), actionInput(observation, 'press', targetRef!));

  expect(prepared.focused).toBe(true);
  await expect(page.frameLocator('#frame').locator('#target')).toBeFocused();
  expect(await page.locator('#frame').evaluate(element => (
    element.ownerDocument.activeElement === element
  ))).toBe(true);
});

test('authenticated trusted input does not masquerade as user takeover state', async ({ page }) => {
  await page.setContent('<!doctype html><button id="target">Open details</button>');
  await page.addScriptTag({ content: runtimeSource });
  await page.addScriptTag({ content: takeoverSource });
  const before = await observe(page);
  const targetBounds = await page.locator('#target').boundingBox();
  if (!targetBounds) throw new Error('Expected trusted input target bounds');
  const expectedPointer = {
    ...await targetBinding(page, '#target'),
    kind: 'pointer' as const,
    x: targetBounds.x + targetBounds.width / 2,
    y: targetBounds.y + targetBounds.height / 2,
    button: 'left',
  };

  expect(await page.evaluate((expected) => (
    window as unknown as { __NEXA_TRUSTED_INPUT_GUARD__: TrustedInputGuard }
  ).__NEXA_TRUSTED_INPUT_GUARD__.arm(
    'playwright-takeover-token',
    'operation-1',
    { pointerDown: 1, keyDown: 0, input: 0 },
    expected,
  ), expectedPointer)).toBe(true);
  await page.locator('#target').click();
  const guarded = await observe(page);
  expect(guarded.userEpoch).toBe(before.userEpoch);

  expect(await page.evaluate(({ expected }) => (
    window as unknown as { __NEXA_TRUSTED_INPUT_GUARD__: TrustedInputGuard }
  ).__NEXA_TRUSTED_INPUT_GUARD__.arm(
    'playwright-takeover-token',
    'operation-2',
    { pointerDown: 1, keyDown: 0, input: 0 },
    { ...expected, x: expected.x + 100 },
  ), { expected: expectedPointer })).toBe(false);
  await page.locator('#target').click();
  const unguarded = await observe(page);
  expect(unguarded.userEpoch).toBe(before.userEpoch + 1);
});

test('trusted pointer guard rejects a replacement element at the approved coordinates', async ({ page }) => {
  await page.setContent('<!doctype html><button id="target">Approved target</button>');
  await page.addScriptTag({ content: runtimeSource });
  await page.addScriptTag({ content: takeoverSource });
  const before = await observe(page);
  const targetBounds = await page.locator('#target').boundingBox();
  if (!targetBounds) throw new Error('Expected trusted input target bounds');
  const expected = {
    ...await targetBinding(page, '#target'),
    kind: 'pointer' as const,
    x: targetBounds.x + targetBounds.width / 2,
    y: targetBounds.y + targetBounds.height / 2,
    button: 'left' as const,
  };

  expect(await page.evaluate((pointer) => (
    window as unknown as { __NEXA_TRUSTED_INPUT_GUARD__: TrustedInputGuard }
  ).__NEXA_TRUSTED_INPUT_GUARD__.arm(
    'playwright-takeover-token',
    'replacement-operation',
    { pointerDown: 1, keyDown: 0, input: 0 },
    pointer,
  ), expected)).toBe(true);
  await page.evaluate(() => {
    const original = document.querySelector('#target');
    const replacement = document.createElement('button');
    replacement.id = 'replacement';
    replacement.textContent = 'Different action';
    original?.replaceWith(replacement);
  });
  await page.locator('#replacement').click();

  expect((await observe(page)).userEpoch).toBe(before.userEpoch + 1);
});

test('trusted text and key guards require the exact dispatched event signature', async ({ page }) => {
  await page.setContent('<!doctype html><input id="target"><button id="button">Continue</button>');
  await page.addScriptTag({ content: runtimeSource });
  await page.addScriptTag({ content: takeoverSource });
  await page.locator('#target').focus();
  const beforeText = await observe(page);

  expect(await page.evaluate((binding) => (
    window as unknown as { __NEXA_TRUSTED_INPUT_GUARD__: TrustedInputGuard }
  ).__NEXA_TRUSTED_INPUT_GUARD__.arm(
    'playwright-takeover-token',
    'text-operation',
    { pointerDown: 0, keyDown: 0, input: 1 },
    { kind: 'text', data: 'agent text', ...binding },
  ), await targetBinding(page, '#target'))).toBe(true);
  await page.keyboard.insertText('agent text');
  expect((await observe(page)).userEpoch).toBe(beforeText.userEpoch);

  await page.locator('#button').focus();
  const beforeKey = await observe(page);
  expect(await page.evaluate((binding) => (
    window as unknown as { __NEXA_TRUSTED_INPUT_GUARD__: TrustedInputGuard }
  ).__NEXA_TRUSTED_INPUT_GUARD__.arm(
    'playwright-takeover-token',
    'key-operation',
    { pointerDown: 0, keyDown: 1, input: 0 },
    { kind: 'key', key: 'Escape', ...binding },
  ), await targetBinding(page, '#button'))).toBe(true);
  await page.keyboard.press('Escape');
  expect((await observe(page)).userEpoch).toBe(beforeKey.userEpoch);

  expect(await page.evaluate((binding) => (
    window as unknown as { __NEXA_TRUSTED_INPUT_GUARD__: TrustedInputGuard }
  ).__NEXA_TRUSTED_INPUT_GUARD__.arm(
    'playwright-takeover-token',
    'mismatched-text-operation',
    { pointerDown: 0, keyDown: 0, input: 1 },
    { kind: 'text', data: 'agent text', ...binding },
  ), await targetBinding(page, '#button'))).toBe(true);
  await page.locator('#target').focus();
  await page.keyboard.insertText('user text');
  expect((await observe(page)).userEpoch).toBe(beforeKey.userEpoch + 1);
});

async function observe(page: import('@playwright/test').Page): Promise<BrowserObservation> {
  return page.evaluate(() => (
    window as unknown as { __NEXA_BROWSER_RUNTIME__: BrowserBridge }
  ).__NEXA_BROWSER_RUNTIME__.observe());
}

async function targetBinding(page: import('@playwright/test').Page, selector: string) {
  return page.evaluate(selector => {
    const bridge = (window as unknown as { __NEXA_BROWSER_RUNTIME__: BrowserBridge }).__NEXA_BROWSER_RUNTIME__;
    const element = document.querySelector(selector)!;
    const observation = bridge.observe();
    return {
      targetRef: observation.elements.find(item => bridge.resolveTargetRef(item.ref) === element)!.ref,
      targetContext: bridge.targetContextFingerprint(element),
    };
  }, selector);
}

function actionInput(
  observation: BrowserObservation,
  action: string,
  targetRef: string,
  endRef?: string,
): BrowserActionInput {
  return {
    action,
    targetRef,
    endRef,
    button: 'left',
    modifiers: [],
    userEpoch: observation.userEpoch,
    domFingerprint: observation.domFingerprint,
    interactionFingerprint: observation.interactionFingerprint,
    expected: observation.elements.find(element => element.ref === targetRef),
    expectedEnd: observation.elements.find(element => element.ref === endRef),
  };
}

interface BrowserElement {
  ref: string;
  name: string;
  bounds: { x: number; y: number; width: number; height: number };
}

interface BrowserObservation {
  url: string;
  userEpoch: number;
  domFingerprint: string;
  interactionFingerprint: string;
  elements: BrowserElement[];
}

interface BrowserVerificationBaseline {
  url: string;
  userEpoch: number;
  domFingerprint: string;
}

interface BrowserActionInput {
  action: string;
  targetRef: string;
  endRef?: string;
  button: string;
  modifiers: string[];
  userEpoch: number;
  domFingerprint: string;
  interactionFingerprint: string;
  expected?: BrowserElement;
  expectedEnd?: BrowserElement;
}

interface BrowserBridge {
  targetContextFingerprint(element: Element): string;
  resolveTargetRef(ref: string): Element | null;
  observe(): BrowserObservation;
  previewAction(input: BrowserActionInput): { durationMs: number };
  prepareNativePointer(input: BrowserActionInput): {
    bounds: { x: number; y: number; width: number; height: number };
    verificationBaseline: BrowserVerificationBaseline;
  };
  prepareTrustedText(input: BrowserActionInput): {
    focused: boolean;
    verificationBaseline: BrowserVerificationBaseline;
  };
  prepareTrustedKey(input: BrowserActionInput): {
    focused: boolean;
    verificationBaseline: BrowserVerificationBaseline;
  };
  act(input: BrowserActionInput): boolean;
  invalidateForUserTakeover(): void;
}

function snapshotChanged(
  before: BrowserVerificationBaseline,
  after: BrowserObservation,
): boolean {
  return before.url !== after.url
    || before.domFingerprint !== after.domFingerprint
    || before.userEpoch !== after.userEpoch;
}

interface TrustedInputGuard {
  arm(
    token: string,
    operationId: string,
    budget: { pointerDown: number; keyDown: number; input: number },
    expected: ({
      kind: 'pointer';
      x: number;
      y: number;
      button: 'left' | 'middle' | 'right';
    } | { kind: 'text'; data: string } | { kind: 'key'; key: string }) & { targetRef: string; targetContext: string },
  ): boolean;
}
