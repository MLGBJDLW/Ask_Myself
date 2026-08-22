import assert from 'node:assert/strict';
import fs from 'node:fs';
import path from 'node:path';
import test from 'node:test';
import { fileURLToPath } from 'node:url';

const repositoryRoot = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '..');
const releaseWorkflow = fs.readFileSync(
  path.join(repositoryRoot, '.github', 'workflows', 'release.yml'),
  'utf8',
);
const nativeAcceptanceWorkflow = fs.readFileSync(
  path.join(repositoryRoot, '.github', 'workflows', 'office-native-acceptance.yml'),
  'utf8',
);

test('release runs SHA-bound native Office acceptance before building artifacts', () => {
  assert.match(nativeAcceptanceWorkflow, /^  workflow_call:\s*$/m);
  assert.match(nativeAcceptanceWorkflow, /target_ref:[\s\S]*?required: true[\s\S]*?type: string/);
  assert.match(nativeAcceptanceWorkflow, /target_sha:[\s\S]*?required: true[\s\S]*?type: string/);

  assert.match(releaseWorkflow, /^  native-office-acceptance:\s*$/m);
  assert.match(
    releaseWorkflow,
    /uses: \.\/\.github\/workflows\/office-native-acceptance\.yml/,
  );
  assert.match(
    releaseWorkflow,
    /target_ref: \$\{\{ needs\.release-please\.outputs\.target_ref \}\}/,
  );
  assert.match(
    releaseWorkflow,
    /target_sha: \$\{\{ needs\.release-please\.outputs\.target_sha \}\}/,
  );
  assert.match(
    releaseWorkflow,
    /needs: \[release-please, preflight, native-office-acceptance\]/,
  );
  assert.doesNotMatch(
    releaseWorkflow,
    /actions\/workflows\/office-native-acceptance\.yml\/runs/,
  );
});

test('manual dispatch can resume an existing draft release without creating a new tag', () => {
  assert.match(releaseWorkflow, /^      release_tag:\s*$/m);
  assert.match(
    releaseWorkflow,
    /id: release\s+if: github\.event_name != 'workflow_dispatch' \|\| inputs\.release_tag == ''/,
  );
  assert.match(
    releaseWorkflow,
    /id: resume\s+if: github\.event_name == 'workflow_dispatch' && inputs\.release_tag != ''/,
  );
  assert.match(releaseWorkflow, /isDraft/);
  assert.match(releaseWorkflow, /Release .* is not a draft release/);
  assert.match(releaseWorkflow, /compare\/\$target_sha\.\.\.master/);
  assert.match(releaseWorkflow, /Release target .* is not an ancestor of master/);
  assert.match(releaseWorkflow, /manifest_version/);
  assert.match(releaseWorkflow, /Release tag .* moved from .* to/);
  assert.match(releaseWorkflow, /steps\.resume\.outputs\.tag_name/);
  assert.match(releaseWorkflow, /steps\.resume\.outputs\.version/);
  assert.match(releaseWorkflow, /steps\.resume\.outputs\.target_sha/);
  assert.match(
    releaseWorkflow,
    /ref: \$\{\{ needs\.release-please\.outputs\.target_sha \}\}/,
  );
});

test('release PR CI dispatch does not depend on a local git checkout', () => {
  assert.match(releaseWorkflow, /GH_REPO: \$\{\{ github\.repository \}\}/);
  assert.match(
    releaseWorkflow,
    /gh pr list --state open --head "\$RELEASE_PR_BRANCH"/,
  );
  assert.doesNotMatch(releaseWorkflow, /"\$OWNER:\$RELEASE_PR_BRANCH"/);
  assert.match(
    releaseWorkflow,
    /gh api --method POST[\s\S]*?repos\/\$GITHUB_REPOSITORY\/actions\/workflows\/ci\.yml\/dispatches[\s\S]*?-f ref="\$RELEASE_PR_BRANCH"/,
  );
  assert.doesNotMatch(releaseWorkflow, /gh workflow run ci\.yml/);
});
