#!/usr/bin/env node
/**
 * Nexa icon build pipeline.
 *
 * Generates all Tauri-required raster icons from the master SVG.
 *
 * Usage:
 *   1. Install dev deps (one-time):
 *        npm install -D @resvg/resvg-js png2icons  (run in repo root or apps/desktop)
 *   2. Run:
 *        node scripts/build-icons.mjs
 *
 * Inputs:  apps/desktop/src-tauri/icons/icon.svg
 * Outputs: 32x32.png, 128x128.png, 128x128@2x.png, icon.png,
 *          Square30x30Logo.png ... Square310x310Logo.png, StoreLogo.png,
 *          icon.icns, icon.ico
 */

import { readFile, writeFile } from 'node:fs/promises';
import { resolve, dirname } from 'node:path';
import { fileURLToPath } from 'node:url';
import { Resvg } from '@resvg/resvg-js';
import png2icons from 'png2icons';

const __dirname = dirname(fileURLToPath(import.meta.url));
const ROOT = resolve(__dirname, '..');
const ICONS_DIR = resolve(ROOT, 'apps/desktop/src-tauri/icons');
const SVG_PATH = resolve(ICONS_DIR, 'icon.svg');

async function renderPng(svgBuf, size) {
  const resvg = new Resvg(svgBuf, { fitTo: { mode: 'width', value: size } });
  return resvg.render().asPng();
}

async function main() {
  const svg = await readFile(SVG_PATH);

  // Core Tauri sizes + Windows Store tiles
  const sizes = {
    '32x32.png': 32,
    '128x128.png': 128,
    '128x128@2x.png': 256,
    'icon.png': 512,
    'Square30x30Logo.png': 30,
    'Square44x44Logo.png': 44,
    'Square71x71Logo.png': 71,
    'Square89x89Logo.png': 89,
    'Square107x107Logo.png': 107,
    'Square142x142Logo.png': 142,
    'Square150x150Logo.png': 150,
    'Square284x284Logo.png': 284,
    'Square310x310Logo.png': 310,
    'StoreLogo.png': 50,
  };

  for (const [name, size] of Object.entries(sizes)) {
    const png = await renderPng(svg, size);
    await writeFile(resolve(ICONS_DIR, name), png);
    console.log(`\u2713 ${name} (${size}\u00d7${size})`);
  }

  // Multi-size source for .icns / .ico
  const base512 = await renderPng(svg, 512);

  // ICNS (macOS)
  const icns = png2icons.createICNS(base512, png2icons.BILINEAR, 0);
  if (icns) {
    await writeFile(resolve(ICONS_DIR, 'icon.icns'), icns);
    console.log('\u2713 icon.icns');
  }

  // ICO (Windows)
  const ico = png2icons.createICO(base512, png2icons.BILINEAR, 0, false, true);
  if (ico) {
    await writeFile(resolve(ICONS_DIR, 'icon.ico'), ico);
    console.log('\u2713 icon.ico');
  }

  console.log('\nAll icons generated.');
}

main().catch((err) => {
  console.error(err);
  process.exit(1);
});
