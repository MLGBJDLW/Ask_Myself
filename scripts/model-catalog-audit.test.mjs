import assert from 'node:assert/strict';
import { compareEndpointModels, driftDetected } from './model-catalog-audit-lib.mjs';

const endpoint = {
  endpointId: 'text:tenant-a',
  models: [{
    id: 'model-a',
    lifecycle: 'active',
    regions: ['us-east'],
    capabilities: { toolCalling: true },
  }],
};

const changed = compareEndpointModels(endpoint, [{
  id: 'model-a',
  status: 'deprecated',
  regions: ['eu-west'],
  capabilities: { toolCalling: false },
}]);
assert.deepEqual(changed.capabilityChanged, [{ id: 'model-a', fields: ['toolCalling'] }]);
assert.deepEqual(changed.lifecycleChanged, [{ id: 'model-a', curated: 'active', discovered: 'deprecated' }]);
assert.equal(changed.regionChanged.length, 1);
assert.equal(driftDetected(changed), true);

const isolated = compareEndpointModels(endpoint, [{ id: 'tenant-b-only-model' }]);
assert.deepEqual(isolated.newIds, ['tenant-b-only-model']);
assert.deepEqual(isolated.missingIds, ['model-a']);
assert.equal(isolated.capabilityChanged.length, 0);

const clean = compareEndpointModels(endpoint, [{
  id: 'model-a',
  regions: ['us-east'],
  capabilities: { toolCalling: true },
}]);
assert.equal(driftDetected(clean), false);
process.stdout.write('model catalog audit fixtures passed\n');
