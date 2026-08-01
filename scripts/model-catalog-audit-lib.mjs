function normalizedValues(values) {
  return [...new Set((Array.isArray(values) ? values : [])
    .map((value) => String(value).trim().toLowerCase())
    .filter(Boolean))].sort();
}

function liveLifecycle(model) {
  const explicit = String(model?.lifecycle ?? model?.status ?? '').trim().toLowerCase();
  if (explicit) return explicit;
  return model?.deprecated === true ? 'deprecated' : null;
}

export function compareCapabilities(curated, discovered) {
  if (!discovered || typeof discovered !== 'object') return [];
  return Object.entries(discovered)
    .filter(([key, value]) => Object.hasOwn(curated, key) && curated[key] !== value)
    .map(([key]) => key)
    .sort();
}

/** Compare one endpoint/credential snapshot without crossing identity scopes. */
export function compareEndpointModels(endpoint, liveModels) {
  const liveById = new Map((Array.isArray(liveModels) ? liveModels : [])
    .filter((model) => typeof model?.id === 'string')
    .map((model) => [model.id, model]));
  const curatedById = new Map(endpoint.models.map((model) => [model.id, model]));
  const newIds = [...liveById.keys()].filter((id) => !curatedById.has(id)).sort();
  const missingIds = [...curatedById.keys()].filter((id) => !liveById.has(id)).sort();
  const capabilityChanged = [];
  const lifecycleChanged = [];
  const regionChanged = [];

  for (const [id, curated] of curatedById) {
    const live = liveById.get(id);
    if (!live) continue;
    const changed = compareCapabilities(curated.capabilities, live.capabilities);
    if (changed.length) capabilityChanged.push({ id, fields: changed });
    const lifecycle = liveLifecycle(live);
    if (lifecycle && lifecycle !== curated.lifecycle) {
      lifecycleChanged.push({ id, curated: curated.lifecycle, discovered: lifecycle });
    }
    if (Array.isArray(live.regions)) {
      const curatedRegions = normalizedValues(curated.regions);
      const discoveredRegions = normalizedValues(live.regions);
      if (JSON.stringify(curatedRegions) !== JSON.stringify(discoveredRegions)) {
        regionChanged.push({ id, curated: curatedRegions, discovered: discoveredRegions });
      }
    }
  }

  return { newIds, missingIds, capabilityChanged, lifecycleChanged, regionChanged };
}

export function driftDetected(comparison) {
  return comparison.newIds.length > 0
    || comparison.missingIds.length > 0
    || comparison.capabilityChanged.length > 0
    || comparison.lifecycleChanged.length > 0
    || comparison.regionChanged.length > 0;
}
