import { mkdir, readFile, writeFile } from 'node:fs/promises';
import path from 'node:path';
import process from 'node:process';
import { fileURLToPath } from 'node:url';
import {
  compareEndpointModels,
  compareRequiredModelIds,
  driftDetected,
} from './model-catalog-audit-lib.mjs';

const repoRoot = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '..');
const livePublicCatalogs = process.argv.includes('--live-public');
const outputFlag = process.argv.indexOf('--output');
const outputDir = path.resolve(
  repoRoot,
  outputFlag >= 0 && process.argv[outputFlag + 1]
    ? process.argv[outputFlag + 1]
    : '.artifacts/model-catalog-drift',
);

const sources = [
  ['text', 'shared/provider-presets.json'],
  ['image', 'shared/image-provider-presets.json'],
  ['embedding', 'shared/embedding-provider-presets.json'],
  ['speech_to_text', 'shared/stt-provider-presets.json'],
  ['text_to_speech', 'shared/tts-provider-presets.json'],
];

const discoveryCredentials = new Map([
  ['text:openai', 'NEXA_OPENAI_CATALOG_API_KEY'],
  ['text:openrouter', 'NEXA_OPENROUTER_CATALOG_API_KEY'],
  ['text:deepseek', 'NEXA_DEEPSEEK_CATALOG_API_KEY'],
  ['text:alibaba-model-studio', 'NEXA_ALIBABA_CATALOG_API_KEY'],
  ['text:qwen-cloud-intl', 'NEXA_ALIBABA_INTL_CATALOG_API_KEY'],
]);

// OpenRouter's first-party `/models` endpoint is public. Limit the anonymous
// contract to exact integration-critical IDs so Nexa's curated picker is not
// mistaken for an exhaustive mirror of the router's full catalog.
const publicRequiredModelIds = new Map([
  ['text:openrouter', ['z-ai/glm-5.3', 'z-ai/glm-5.3-flash']],
]);

const readJson = async (relativePath) => JSON.parse(
  await readFile(path.join(repoRoot, relativePath), 'utf8'),
);

function normalizedUrl(value) {
  return String(value ?? '').trim().replace(/\/+$/, '');
}

function inferredLifecycle(model) {
  const explicit = String(model.status ?? '').toLowerCase();
  if (explicit) return explicit;
  return /preview/i.test(`${model.id} ${model.name} ${model.tagKey ?? ''}`) ? 'preview' : 'active';
}

function inferredAccess(model, lifecycle) {
  if (model.access) return model.access;
  if (lifecycle === 'preview') return 'application';
  if (lifecycle === 'gated') return 'account_enablement';
  return 'public';
}

function inferredReadiness(model) {
  if (model.productReadiness) return model.productReadiness;
  if (model.source === 'discovered') return 'discoverable';
  return 'known';
}

function modelCapabilities(model) {
  return {
    ...(model.capabilities ?? {}),
    ...(typeof model.supportsTools === 'boolean' ? { toolCalling: model.supportsTools } : {}),
    ...(typeof model.supportsStructuredOutput === 'boolean'
      ? { structuredOutput: model.supportsStructuredOutput }
      : {}),
    ...(typeof model.supportsDimensionOverride === 'boolean'
      ? { dimensionOverride: model.supportsDimensionOverride }
      : {}),
  };
}

async function buildStaticCatalog() {
  const schema = await readJson('shared/model-catalog.schema.json');
  const lifecycleValues = new Set(schema.$defs.modelLifecycle.enum);
  const accessValues = new Set(schema.$defs.modelAccess.enum);
  const readinessValues = new Set(schema.$defs.productReadiness.enum);
  const errors = [];
  const endpoints = [];
  const endpointIds = new Set();

  for (const [surface, relativePath] of sources) {
    const presets = await readJson(relativePath);
    for (const preset of presets) {
      const endpointId = `${surface}:${preset.id}`;
      if (endpointIds.has(endpointId)) errors.push(`duplicate endpoint id: ${endpointId}`);
      endpointIds.add(endpointId);
      const seenModels = new Set();
      const models = [];
      for (const model of preset.models ?? []) {
        if (!model.id || !model.name) errors.push(`${endpointId}: model id and name are required`);
        if (seenModels.has(model.id)) errors.push(`${endpointId}: duplicate model id ${model.id}`);
        seenModels.add(model.id);
        const lifecycle = inferredLifecycle(model);
        const access = inferredAccess(model, lifecycle);
        const productReadiness = inferredReadiness(model);
        if (!lifecycleValues.has(lifecycle)) errors.push(`${endpointId}/${model.id}: invalid lifecycle ${lifecycle}`);
        if (!accessValues.has(access)) errors.push(`${endpointId}/${model.id}: invalid access ${access}`);
        if (!readinessValues.has(productReadiness)) errors.push(`${endpointId}/${model.id}: invalid readiness ${productReadiness}`);
        models.push({
          id: model.id,
          aliases: model.aliases ?? [],
          lifecycle,
          regions: model.regions ?? preset.regions ?? [],
          capabilities: modelCapabilities(model),
        });
      }
      endpoints.push({
        endpointId,
        baseUrl: normalizedUrl(preset.baseUrl),
        apiStyle: preset.apiStyle ?? (surface === 'text' ? 'openai_chat' : ''),
        models,
      });
    }
  }
  return { endpoints, errors };
}

async function probeEndpoint(endpoint) {
  const credentialEnv = discoveryCredentials.get(endpoint.endpointId);
  const credential = credentialEnv ? process.env[credentialEnv]?.trim() : '';
  const requiredIds = livePublicCatalogs
    ? publicRequiredModelIds.get(endpoint.endpointId) ?? []
    : [];
  const anonymousRequiredIdsOnly = !credential && requiredIds.length > 0;
  if ((!credentialEnv || !credential) && !anonymousRequiredIdsOnly) {
    return { endpointId: endpoint.endpointId, status: 'skipped', reason: 'credential_not_configured' };
  }
  if (!endpoint.baseUrl || !String(endpoint.apiStyle).startsWith('openai')) {
    return { endpointId: endpoint.endpointId, status: 'skipped', reason: 'discovery_not_supported' };
  }

  try {
    const response = await fetch(`${endpoint.baseUrl}/models`, {
      headers: credential ? { Authorization: `Bearer ${credential}` } : undefined,
      signal: AbortSignal.timeout(15_000),
    });
    if (!response.ok) {
      return { endpointId: endpoint.endpointId, status: 'error', reason: `http_${response.status}` };
    }
    const payload = await response.json();
    const liveModels = Array.isArray(payload?.data)
      ? payload.data
      : Array.isArray(payload?.models)
        ? payload.models
        : [];
    const comparison = anonymousRequiredIdsOnly
      ? compareRequiredModelIds(requiredIds, liveModels)
      : compareEndpointModels(endpoint, liveModels);
    return {
      endpointId: endpoint.endpointId,
      status: 'ok',
      ...(anonymousRequiredIdsOnly ? { probeMode: 'required_ids', requiredIds } : {}),
      discoveredCount: liveModels.length,
      ...comparison,
    };
  } catch (error) {
    const reason = error instanceof Error && error.name === 'TimeoutError' ? 'timeout' : 'request_failed';
    return { endpointId: endpoint.endpointId, status: 'error', reason };
  }
}

function markdownReport(report) {
  const lines = [
    '# Model Catalog Drift Report',
    '',
    `Generated: ${report.generatedAt}`,
    '',
    `Static validation: ${report.staticValidation.errors.length ? 'failed' : 'passed'} (${report.staticValidation.endpointCount} endpoints, ${report.staticValidation.modelCount} model projections)`,
    '',
  ];
  if (report.staticValidation.errors.length) {
    lines.push('## Static validation errors', '', ...report.staticValidation.errors.map((error) => `- ${error}`), '');
  }
  lines.push('## Discovery probes', '');
  for (const probe of report.probes) {
    lines.push(`### ${probe.endpointId}`, '', `Status: ${probe.status}`);
    if (probe.reason) lines.push(`Reason: ${probe.reason}`);
    if (probe.status === 'ok') {
      if (probe.probeMode === 'required_ids') {
        lines.push(`Required IDs: ${probe.requiredIds.join(', ')}`);
      }
      lines.push(
        `Discovered: ${probe.discoveredCount}`,
        `New: ${probe.newIds.length ? probe.newIds.join(', ') : 'none'}`,
        `Missing: ${probe.missingIds.length ? probe.missingIds.join(', ') : 'none'}`,
        `Capability changes: ${probe.capabilityChanged.length ? probe.capabilityChanged.map((item) => `${item.id} (${item.fields.join(', ')})`).join('; ') : 'none'}`,
        `Lifecycle changes: ${probe.lifecycleChanged.length ? probe.lifecycleChanged.map((item) => `${item.id} (${item.curated} → ${item.discovered})`).join('; ') : 'none'}`,
        `Region changes: ${probe.regionChanged.length ? probe.regionChanged.map((item) => `${item.id} (${item.curated.join('+')} → ${item.discovered.join('+')})`).join('; ') : 'none'}`,
      );
    }
    lines.push('');
  }
  lines.push('No credentials, request headers, or provider response bodies are stored in this report.', '');
  return lines.join('\n');
}

const staticCatalog = await buildStaticCatalog();
const probes = [];
for (const endpoint of staticCatalog.endpoints) {
  if (
    discoveryCredentials.has(endpoint.endpointId)
    || (livePublicCatalogs && publicRequiredModelIds.has(endpoint.endpointId))
  ) {
    probes.push(await probeEndpoint(endpoint));
  }
}
const modelCount = staticCatalog.endpoints.reduce((total, endpoint) => total + endpoint.models.length, 0);
const completedProbeCount = probes.filter((probe) => probe.status === 'ok').length;
const attemptedProbeCount = probes.filter((probe) => probe.status !== 'skipped').length;
const failedProbeCount = probes.filter((probe) => probe.status === 'error').length;
const allAttemptedProbesSucceeded = attemptedProbeCount > 0 && failedProbeCount === 0;
const hasDrift = staticCatalog.errors.length > 0
  || failedProbeCount > 0
  || probes.some((probe) => probe.status === 'ok' && driftDetected(probe));
const report = {
  schemaVersion: 1,
  generatedAt: new Date().toISOString(),
  hasDrift,
  completedProbeCount,
  attemptedProbeCount,
  failedProbeCount,
  allAttemptedProbesSucceeded,
  staticValidation: {
    endpointCount: staticCatalog.endpoints.length,
    modelCount,
    errors: staticCatalog.errors,
  },
  probes,
};

await mkdir(outputDir, { recursive: true });
await Promise.all([
  writeFile(path.join(outputDir, 'report.json'), `${JSON.stringify(report, null, 2)}\n`),
  writeFile(path.join(outputDir, 'report.md'), markdownReport(report)),
]);
process.stdout.write(`${JSON.stringify({ outputDir, hasDrift, staticErrors: staticCatalog.errors.length })}\n`);
if (staticCatalog.errors.length) process.exitCode = 1;
