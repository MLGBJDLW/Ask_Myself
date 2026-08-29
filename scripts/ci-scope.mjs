import fs from 'node:fs';
import path from 'node:path';
import { execFileSync } from 'node:child_process';
import { fileURLToPath } from 'node:url';

import { synchronizePackageVersion } from './sync-workspace-lock-versions.mjs';

export const RELEASE_PLEASE_BRANCH =
  'release-please--branches--master--components--nexa-monorepo';

export const RELEASE_METADATA_PATHS = new Set([
  '.release-please-manifest.json',
  'CHANGELOG.md',
  'Cargo.lock',
  'apps/desktop/package.json',
  'apps/desktop/src-tauri/Cargo.toml',
  'apps/desktop/src-tauri/tauri.conf.json',
  'crates/core/Cargo.toml',
  'package-lock.json',
  'package.json',
]);

const VERSION_SENTINEL = '0.0.0-nexa-release-metadata';

const JSON_VERSION_PATHS = new Map([
  ['.release-please-manifest.json', [['.']]],
  ['apps/desktop/package.json', [['version']]],
  ['apps/desktop/src-tauri/tauri.conf.json', [['version']]],
  ['package-lock.json', [['version'], ['packages', '', 'version']]],
  ['package.json', [['version']]],
]);

function normalizeJsonVersionFields(changedPath, contents) {
  const parsed = JSON.parse(contents);
  for (const propertyPath of JSON_VERSION_PATHS.get(changedPath) ?? []) {
    let owner = parsed;
    for (const key of propertyPath.slice(0, -1)) {
      owner = owner?.[key];
    }
    const key = propertyPath.at(-1);
    if (!owner || typeof owner[key] !== 'string') {
      throw new Error(`${changedPath} is missing ${propertyPath.join('.')}`);
    }
    owner[key] = VERSION_SENTINEL;
  }
  return JSON.stringify(parsed);
}

function normalizeCargoManifestVersion(contents) {
  const packageHeader = /^\[package\][ \t]*$(?:\r?\n)?/mu.exec(contents);
  if (!packageHeader) {
    throw new Error('Cargo manifest is missing [package]');
  }
  const sectionStart = packageHeader.index + packageHeader[0].length;
  const remainder = contents.slice(sectionStart);
  const nextSection = /^\[/mu.exec(remainder);
  const sectionEnd = sectionStart + (nextSection?.index ?? remainder.length);
  const packageSection = contents.slice(sectionStart, sectionEnd);
  let replacements = 0;
  const normalizedSection = packageSection.replace(
    /^version[ \t]*=[ \t]*"[^"]+"[ \t]*$/gmu,
    () => {
      replacements += 1;
      return `version = "${VERSION_SENTINEL}"`;
    },
  );
  if (replacements !== 1) {
    throw new Error(`Cargo manifest must contain one [package].version; found ${replacements}`);
  }
  return `${contents.slice(0, sectionStart)}${normalizedSection}${contents.slice(sectionEnd)}`;
}

function normalizeWorkspaceLockVersions(contents) {
  return ['nexa-core', 'nexa-desktop'].reduce(
    (normalized, packageName) =>
      synchronizePackageVersion(normalized, packageName, VERSION_SENTINEL),
    contents,
  );
}

export function releaseMetadataContentsMatch(changedPath, baseContents, headContents) {
  try {
    if (changedPath === 'CHANGELOG.md') {
      return true;
    }
    if (JSON_VERSION_PATHS.has(changedPath)) {
      return (
        normalizeJsonVersionFields(changedPath, baseContents) ===
        normalizeJsonVersionFields(changedPath, headContents)
      );
    }
    if (
      changedPath === 'apps/desktop/src-tauri/Cargo.toml' ||
      changedPath === 'crates/core/Cargo.toml'
    ) {
      return (
        normalizeCargoManifestVersion(baseContents) ===
        normalizeCargoManifestVersion(headContents)
      );
    }
    if (changedPath === 'Cargo.lock') {
      return (
        normalizeWorkspaceLockVersions(baseContents) ===
        normalizeWorkspaceLockVersions(headContents)
      );
    }
  } catch {
    return false;
  }
  return false;
}

export function isReleaseMetadataOnly({
  branch,
  headRepository,
  repository,
  changedPaths,
  readBaseFile,
  readHeadFile,
}) {
  if (
    branch !== RELEASE_PLEASE_BRANCH ||
    headRepository !== repository ||
    changedPaths.length === 0 ||
    !changedPaths.every((changedPath) => RELEASE_METADATA_PATHS.has(changedPath)) ||
    typeof readBaseFile !== 'function' ||
    typeof readHeadFile !== 'function'
  ) {
    return false;
  }
  return changedPaths.every((changedPath) => {
    try {
      return releaseMetadataContentsMatch(
        changedPath,
        readBaseFile(changedPath),
        readHeadFile(changedPath),
      );
    } catch {
      return false;
    }
  });
}

const isCli =
  process.argv[1] &&
  path.resolve(process.argv[1]) === path.resolve(fileURLToPath(import.meta.url));

if (isCli) {
  const changedPaths = fs
    .readFileSync(0, 'utf8')
    .split(/\r?\n/u)
    .filter(Boolean);
  const baseRef = process.env.NEXA_CI_BASE_REF ?? 'origin/master';
  const headRef = process.env.NEXA_CI_HEAD_REF ?? 'HEAD';
  const readGitFile = (ref, changedPath) =>
    execFileSync('git', ['show', `${ref}:${changedPath}`], {
      encoding: 'utf8',
      stdio: ['ignore', 'pipe', 'pipe'],
    });
  const result = isReleaseMetadataOnly({
    branch: process.env.NEXA_CI_BRANCH ?? '',
    headRepository: process.env.NEXA_CI_HEAD_REPOSITORY ?? '',
    repository: process.env.GITHUB_REPOSITORY ?? '',
    changedPaths,
    readBaseFile: (changedPath) => readGitFile(baseRef, changedPath),
    readHeadFile: (changedPath) => readGitFile(headRef, changedPath),
  });
  process.stdout.write(result ? 'true' : 'false');
}
