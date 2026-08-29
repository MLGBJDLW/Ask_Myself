import assert from 'node:assert/strict';
import test from 'node:test';

import { synchronizePackageVersion } from './sync-workspace-lock-versions.mjs';

test('updates only the selected workspace package block', () => {
  const source = `version = 4

[[package]]
name = "dependency"
version = "9.8.7"

[[package]]
name = "nexa-core"
version = "0.1.0"
dependencies = [
 "dependency",
]
`;
  const updated = synchronizePackageVersion(source, 'nexa-core', '0.2.0');
  assert.match(updated, /name = "nexa-core"\nversion = "0\.2\.0"/u);
  assert.match(updated, /version = "0\.2\.0"\ndependencies = \[/u);
  assert.match(updated, /name = "dependency"\nversion = "9\.8\.7"/u);
});

test('updates adjacent workspace packages without consuming block separators', () => {
  const source = `[[package]]
name = "nexa-core"
version = "0.1.0"

[[package]]
name = "nexa-desktop"
version = "0.1.0"
`;
  const coreUpdated = synchronizePackageVersion(source, 'nexa-core', '0.2.0');
  const bothUpdated = synchronizePackageVersion(coreUpdated, 'nexa-desktop', '0.2.0');
  assert.equal(
    bothUpdated,
    `[[package]]
name = "nexa-core"
version = "0.2.0"

[[package]]
name = "nexa-desktop"
version = "0.2.0"
`,
  );
});

test('fails closed when a requested package is absent', () => {
  assert.throws(
    () => synchronizePackageVersion('version = 4\n', 'nexa-core', '0.2.0'),
    /Expected one Cargo\.lock package named nexa-core; found 0/u,
  );
});
