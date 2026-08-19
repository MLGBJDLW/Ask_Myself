import { readFileSync } from 'node:fs';
import { join } from 'node:path';
import { expect, test } from '@playwright/test';

const source = readFileSync(join(process.cwd(), 'src-tauri', 'src', 'browser', 'scripts.rs'), 'utf8');
const runtimeSource = source.match(/pub const BROWSER_INIT_SCRIPT: &str = r#"\r?\n([\s\S]*?)\r?\n"#;/)?.[1]
  .replace('__NEXA_PICK_TOKEN__', JSON.stringify('playwright-picker-token'));

if (!runtimeSource) throw new Error('Could not extract the native Browser Workspace interaction runtime');

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

async function observe(page: import('@playwright/test').Page): Promise<BrowserObservation> {
  return page.evaluate(() => (
    window as unknown as { __NEXA_BROWSER_RUNTIME__: BrowserBridge }
  ).__NEXA_BROWSER_RUNTIME__.observe());
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
    expected: observation.elements.find(element => element.ref === targetRef),
    expectedEnd: observation.elements.find(element => element.ref === endRef),
  };
}

interface BrowserElement {
  ref: string;
  name: string;
}

interface BrowserObservation {
  userEpoch: number;
  domFingerprint: string;
  elements: BrowserElement[];
}

interface BrowserActionInput {
  action: string;
  targetRef: string;
  endRef?: string;
  button: string;
  modifiers: string[];
  userEpoch: number;
  domFingerprint: string;
  expected?: BrowserElement;
  expectedEnd?: BrowserElement;
}

interface BrowserBridge {
  observe(): BrowserObservation;
  previewAction(input: BrowserActionInput): { durationMs: number };
  prepareNativePointer(input: BrowserActionInput): { bounds: { x: number; y: number; width: number; height: number } };
  act(input: BrowserActionInput): boolean;
  invalidateForUserTakeover(): void;
}
