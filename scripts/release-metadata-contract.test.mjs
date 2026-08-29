import assert from 'node:assert/strict';
import fs from 'node:fs';
import path from 'node:path';
import test from 'node:test';
import { fileURLToPath } from 'node:url';

const repositoryRoot = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '..');

function read(relativePath) {
  return fs.readFileSync(path.join(repositoryRoot, relativePath), 'utf8');
}

function json(relativePath) {
  return JSON.parse(read(relativePath));
}

function cargoVersion(relativePath) {
  const version = read(relativePath).match(
    /^\[package\]\s*$[\s\S]*?^version\s*=\s*"([^"]+)"\s*$/mu,
  )?.[1];
  assert.ok(version, `${relativePath} must declare [package].version`);
  return version;
}

test('release metadata declares one version across every shipped manifest', () => {
  const expected = json('.release-please-manifest.json')['.'];
  const rootPackage = json('package.json');
  const rootLock = json('package-lock.json');
  const versions = new Map([
    ['package.json', rootPackage.version],
    ['package-lock.json', rootLock.version],
    ['package-lock.json root package', rootLock.packages[''].version],
    ['apps/desktop/package.json', json('apps/desktop/package.json').version],
    ['apps/desktop/src-tauri/tauri.conf.json', json('apps/desktop/src-tauri/tauri.conf.json').version],
    ['apps/desktop/src-tauri/Cargo.toml', cargoVersion('apps/desktop/src-tauri/Cargo.toml')],
    ['crates/core/Cargo.toml', cargoVersion('crates/core/Cargo.toml')],
  ]);
  for (const [source, version] of versions) {
    assert.equal(version, expected, `${source} must match release-please manifest`);
  }
});

test('the newest changelog entry matches the release manifest', () => {
  const expected = json('.release-please-manifest.json')['.'];
  const newest = read('CHANGELOG.md').match(/^## \[([^\]]+)\]/mu)?.[1];
  assert.equal(newest, expected);
});
