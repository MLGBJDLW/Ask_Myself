import assert from 'node:assert/strict';
import test from 'node:test';

import {
  RELEASE_METADATA_PATHS,
  RELEASE_PLEASE_BRANCH,
  isReleaseMetadataOnly,
} from './ci-scope.mjs';

const repository = 'MLGBJDLW/Nexa';
const releasePaths = [...RELEASE_METADATA_PATHS];

test('accepts only the repository-owned release-please metadata change set', () => {
  assert.equal(
    isReleaseMetadataOnly({
      branch: RELEASE_PLEASE_BRANCH,
      headRepository: repository,
      repository,
      changedPaths: releasePaths,
    }),
    true,
  );
});

test('rejects an empty change set', () => {
  assert.equal(
    isReleaseMetadataOnly({
      branch: RELEASE_PLEASE_BRANCH,
      headRepository: repository,
      repository,
      changedPaths: [],
    }),
    false,
  );
});

test('rejects a lookalike branch from a fork', () => {
  assert.equal(
    isReleaseMetadataOnly({
      branch: RELEASE_PLEASE_BRANCH,
      headRepository: 'attacker/Nexa',
      repository,
      changedPaths: releasePaths,
    }),
    false,
  );
});

test('rejects an ordinary branch even when it changes only version files', () => {
  assert.equal(
    isReleaseMetadataOnly({
      branch: 'fix/version-files',
      headRepository: repository,
      repository,
      changedPaths: releasePaths,
    }),
    false,
  );
});

test('rejects any non-metadata file on the trusted release branch', () => {
  assert.equal(
    isReleaseMetadataOnly({
      branch: RELEASE_PLEASE_BRANCH,
      headRepository: repository,
      repository,
      changedPaths: [...releasePaths, 'crates/core/src/lib.rs'],
    }),
    false,
  );
});
