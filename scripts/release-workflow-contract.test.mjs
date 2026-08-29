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
const ciWorkflow = fs.readFileSync(
  path.join(repositoryRoot, '.github', 'workflows', 'ci.yml'),
  'utf8',
);
const nativeAcceptanceWorkflowPath = path.join(
  repositoryRoot,
  '.github',
  'workflows',
  'office-native-acceptance.yml',
);

test('release has no self-hosted Office dependency and keeps hosted safety gates', () => {
  assert.equal(fs.existsSync(nativeAcceptanceWorkflowPath), false);
  assert.doesNotMatch(releaseWorkflow, /native-office-acceptance|office-native-acceptance/);
  assert.match(releaseWorkflow, /needs: \[release-please, preflight\]/);
  assert.match(releaseWorkflow, /TAURI_SIGNING_PRIVATE_KEY is required/);
  assert.match(releaseWorkflow, /REQUIRED_UPDATER_PLATFORMS=\(windows-x86_64 linux-x86_64\)/);
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

test('release PR discovery does not depend on a local git checkout', () => {
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

test('release PR maintenance synchronizes lock metadata before dispatching CI', () => {
  const synchronizeAt = releaseWorkflow.indexOf('Synchronize generated Rust lock metadata');
  const dispatchAt = releaseWorkflow.indexOf('Trigger checks for release PR updates');
  assert.ok(synchronizeAt > 0);
  assert.ok(dispatchAt > synchronizeAt);
  assert.match(releaseWorkflow, /node scripts\/sync-workspace-lock-versions\.mjs --write/);
  assert.match(releaseWorkflow, /git add -- Cargo\.lock/);
  assert.match(releaseWorkflow, /steps\.sync_lock\.outputs\.changed/);
});

test('full CI is front-loaded onto pull requests instead of repeated after merge', () => {
  assert.match(ciWorkflow, /^  pull_request:\s*$/mu);
  assert.match(ciWorkflow, /^  workflow_dispatch:\s*$/mu);
  assert.doesNotMatch(ciWorkflow, /^  push:\s*$/mu);
});

test('only the trusted release metadata classifier can select lightweight CI', () => {
  assert.match(ciWorkflow, /name: Classify CI Scope/);
  assert.match(ciWorkflow, /NEXA_CI_BASE_REF: origin\/master/);
  assert.match(ciWorkflow, /NEXA_CI_HEAD_REF: HEAD/);
  assert.match(ciWorkflow, /node scripts\/ci-scope\.mjs/);
  assert.match(ciWorkflow, /name: Release Metadata Contracts/);
  assert.match(
    ciWorkflow,
    /if: needs\.classify\.outputs\.release_metadata_only != 'true'/,
  );
  assert.match(ciWorkflow, /WINDOWS_DESKTOP_RESULT/);
});
