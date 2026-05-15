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
 * Outputs: desktop, Windows Store, iOS, and Android raster icons, plus icon.icns/icon.ico.
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

  const iosSizes = {
    'AppIcon-20x20@1x.png': 20,
    'AppIcon-20x20@2x.png': 40,
    'AppIcon-20x20@2x-1.png': 40,
    'AppIcon-20x20@3x.png': 60,
    'AppIcon-29x29@1x.png': 29,
    'AppIcon-29x29@2x.png': 58,
    'AppIcon-29x29@2x-1.png': 58,
    'AppIcon-29x29@3x.png': 87,
    'AppIcon-40x40@1x.png': 40,
    'AppIcon-40x40@2x.png': 80,
    'AppIcon-40x40@2x-1.png': 80,
    'AppIcon-40x40@3x.png': 120,
    'AppIcon-60x60@2x.png': 120,
    'AppIcon-60x60@3x.png': 180,
    'AppIcon-76x76@1x.png': 76,
    'AppIcon-76x76@2x.png': 152,
    'AppIcon-83.5x83.5@2x.png': 167,
    'AppIcon-512@2x.png': 1024,
  };

  for (const [name, size] of Object.entries(iosSizes)) {
    const png = await renderPng(svg, size);
    await writeFile(resolve(ICONS_DIR, 'ios', name), png);
    console.log(`\u2713 ios/${name} (${size}\u00d7${size})`);
  }

  const androidDensitySizes = {
    'mipmap-mdpi': [48, 108],
    'mipmap-hdpi': [72, 162],
    'mipmap-xhdpi': [96, 216],
    'mipmap-xxhdpi': [144, 324],
    'mipmap-xxxhdpi': [192, 432],
  };

  for (const [density, [launcherSize, foregroundSize]] of Object.entries(androidDensitySizes)) {
    const launcher = await renderPng(svg, launcherSize);
    await writeFile(resolve(ICONS_DIR, 'android', density, 'ic_launcher.png'), launcher);
    await writeFile(resolve(ICONS_DIR, 'android', density, 'ic_launcher_round.png'), launcher);

    const foreground = await renderPng(svg, foregroundSize);
    await writeFile(resolve(ICONS_DIR, 'android', density, 'ic_launcher_foreground.png'), foreground);
    console.log(`\u2713 android/${density} (${launcherSize}\u00d7${launcherSize}, ${foregroundSize}\u00d7${foregroundSize})`);
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
