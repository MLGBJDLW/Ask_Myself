import assert from 'node:assert/strict';
import {
  compareEndpointModels,
  compareRequiredModelIds,
  driftDetected,
} from './model-catalog-audit-lib.mjs';

const endpoint = {
  endpointId: 'text:tenant-a',
  models: [{
    id: 'model-a',
    aliases: ['model-a-latest'],
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

const aliasOnly = compareEndpointModels(endpoint, [{
  id: 'MODEL-A-LATEST',
  regions: ['us-east'],
  capabilities: { toolCalling: true },
}]);
assert.deepEqual(aliasOnly.newIds, []);
assert.deepEqual(aliasOnly.missingIds, []);
assert.equal(driftDetected(aliasOnly), false);

const requiredPublicIds = ['z-ai/glm-5.3', 'z-ai/glm-5.3-flash'];
const publicExact = compareRequiredModelIds(requiredPublicIds, [
  { id: 'unrelated/provider-model' },
  { id: 'Z-AI/GLM-5.3' },
  { id: 'z-ai/glm-5.3-flash' },
]);
assert.deepEqual(publicExact.newIds, []);
assert.deepEqual(publicExact.missingIds, []);
assert.equal(driftDetected(publicExact), false);
const publicMissing = compareRequiredModelIds(requiredPublicIds, [{ id: 'z-ai/glm-5.3' }]);
assert.deepEqual(publicMissing.missingIds, ['z-ai/glm-5.3-flash']);
assert.equal(driftDetected(publicMissing), true);
process.stdout.write('model catalog audit fixtures passed\n');
