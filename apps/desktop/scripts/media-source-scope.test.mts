import test from 'node:test';
import assert from 'node:assert/strict';

import { sourceMayIncludeMedia } from '../src/lib/mediaSourceScope.ts';

function source(includeGlobs: string[]) {
  return {
    id: 'source-1',
    kind: 'local_folder',
    rootPath: 'C:/notes',
    includeGlobs,
    excludeGlobs: [],
    watchEnabled: false,
    createdAt: '',
    updatedAt: '',
  };
}

test('empty and markdown-only sources do not produce speculative media warnings', () => {
  assert.equal(sourceMayIncludeMedia(source([])), false);
  assert.equal(sourceMayIncludeMedia(source(['**/*.md', '**/*.{md,txt}'])), false);
});

test('explicit and broad media-capable filters request media readiness', () => {
  assert.equal(sourceMayIncludeMedia(source(['**/*.{md,mp4}'])), true);
  assert.equal(sourceMayIncludeMedia(source(['**/*'])), true);
});
