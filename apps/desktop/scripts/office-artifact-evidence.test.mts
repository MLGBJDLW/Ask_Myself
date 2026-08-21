import test from 'node:test';
import assert from 'node:assert/strict';

import {
  extractOfficeArtifactEvidence,
  summarizeOfficeArtifactEvidence,
} from '../src/lib/officeArtifactEvidence.ts';

test('normalizes candidate proof including SHA binding and native host evidence', () => {
  const artifactSha256 = 'a'.repeat(64);
  const evidence = extractOfficeArtifactEvidence({
    kind: 'officeArtifactOutcome',
    status: 'candidate',
    sha256: artifactSha256,
    validation: { status: 'pass' },
    preservationEvidence: { verified: true },
    schemaValidation: { status: 'pass' },
    calculationEvidence: { level: 'native', engine: 'microsoft-excel-com' },
    nativeEvidence: {
      engine: 'microsoft-excel-com',
      engineVersion: '16.0',
      nativeOpenSave: true,
    },
    renderEvidence: {
      complete: true,
      artifactSha256,
      renderedSurfaces: 3,
      expectedSurfaces: 3,
    },
    warnings: ['one warning'],
  } as never);

  assert.ok(evidence);
  assert.deepEqual(summarizeOfficeArtifactEvidence(evidence), {
    artifactSha256,
    renderArtifactSha256: artifactSha256,
    renderShaBound: true,
    validationStatus: 'pass',
    preservationStatus: 'pass',
    schemaStatus: 'pass',
    calculationStatus: 'native',
    calculationEngine: 'microsoft-excel-com',
    nativeEngine: 'microsoft-excel-com',
    nativeEngineVersion: '16.0',
    nativeOpenSave: true,
    renderStatus: 'complete',
    renderedSurfaces: 3,
    expectedSurfaces: 3,
    warningCount: 1,
  });
});

test('exposes incomplete or mismatched proof instead of treating it as passing', () => {
  const evidence = extractOfficeArtifactEvidence({
    kind: 'officeArtifactOutcome',
    sha256: 'a'.repeat(64),
    preservationEvidence: { verified: false },
    renderEvidence: {
      complete: false,
      artifactSha256: 'b'.repeat(64),
      renderedSurfaces: 1,
      expectedSurfaces: 2,
    },
  } as never);

  assert.ok(evidence);
  const proof = summarizeOfficeArtifactEvidence(evidence);
  assert.equal(proof.preservationStatus, 'failed');
  assert.equal(proof.renderStatus, 'incomplete');
  assert.equal(proof.renderShaBound, false);
});

test('ignores unrelated tool artifacts', () => {
  assert.equal(extractOfficeArtifactEvidence({ kind: 'browserResult' } as never), null);
});
