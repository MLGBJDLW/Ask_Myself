import fs from 'node:fs';
import path from 'node:path';
import { fileURLToPath } from 'node:url';

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

export function isReleaseMetadataOnly({
  branch,
  headRepository,
  repository,
  changedPaths,
}) {
  return (
    branch === RELEASE_PLEASE_BRANCH &&
    headRepository === repository &&
    changedPaths.length > 0 &&
    changedPaths.every((changedPath) => RELEASE_METADATA_PATHS.has(changedPath))
  );
}

const isCli =
  process.argv[1] &&
  path.resolve(process.argv[1]) === path.resolve(fileURLToPath(import.meta.url));

if (isCli) {
  const changedPaths = fs
    .readFileSync(0, 'utf8')
    .split(/\r?\n/u)
    .filter(Boolean);
  const result = isReleaseMetadataOnly({
    branch: process.env.NEXA_CI_BRANCH ?? '',
    headRepository: process.env.NEXA_CI_HEAD_REPOSITORY ?? '',
    repository: process.env.GITHUB_REPOSITORY ?? '',
    changedPaths,
  });
  process.stdout.write(result ? 'true' : 'false');
}
