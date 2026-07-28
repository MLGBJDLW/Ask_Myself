export interface ProviderCredentialConfig {
  provider: string;
  baseUrl?: string | null;
  apiKey: string;
  name: string;
  isDefault?: boolean;
}

function normalizedUrl(value: string | null | undefined): string {
  return (value ?? '').trim().replace(/\/+$/, '').toLowerCase();
}

function endpointHost(value: string | null | undefined): string {
  const normalized = normalizedUrl(value);
  if (!normalized) return '';
  try {
    return new URL(normalized).hostname;
  } catch {
    return normalized;
  }
}

/**
 * Resolve the credential boundary shared by chat and capability providers.
 *
 * Provider labels are not sufficient here: Alibaba chat uses
 * `alibaba_model_studio`, while its image/TTS catalogs historically use
 * `qwen`. Conversely, Token/Coding Plan keys are endpoint-scoped and must not
 * leak into pay-as-you-go DashScope services.
 */
export function providerCredentialScope(
  provider: string,
  baseUrl?: string | null,
): string {
  const normalizedProvider = provider.trim().toLowerCase();
  const normalizedBaseUrl = normalizedUrl(baseUrl);
  const host = endpointHost(baseUrl);

  if (normalizedBaseUrl.includes('token-plan.') || normalizedBaseUrl.includes('coding.dashscope.')) {
    return `endpoint:${normalizedBaseUrl}`;
  }

  if (host === 'dashscope-intl.aliyuncs.com' || host === 'ap-southeast-1.maas.aliyuncs.com') {
    return 'alibaba-model-studio:singapore';
  }
  if (host === 'dashscope-us.aliyuncs.com' || host === 'us-east-1.maas.aliyuncs.com') {
    return 'alibaba-model-studio:virginia';
  }
  if (host === 'dashscope.aliyuncs.com' || host === 'cn-beijing.maas.aliyuncs.com') {
    return 'alibaba-model-studio:beijing';
  }

  const hostScopes: Array<[string, string]> = [
    ['api.openai.com', 'openai'],
    ['api.anthropic.com', 'anthropic'],
    ['generativelanguage.googleapis.com', 'google'],
    ['api.groq.com', 'groq'],
    ['api.mistral.ai', 'mistral'],
    ['api.minimax.io', 'minimax'],
    ['api.jina.ai', 'jina'],
    ['api.siliconflow.cn', 'siliconflow'],
    ['open.bigmodel.cn', 'zhipu'],
  ];
  const knownHost = hostScopes.find(([candidate]) => host === candidate);
  if (knownHost) return knownHost[1];

  // Unknown or user-edited endpoints are never assumed to share credentials,
  // even when their provider label matches a known vendor.
  if (normalizedBaseUrl) {
    return `endpoint:${normalizedBaseUrl}`;
  }

  const providerAliases: Record<string, string> = {
    alibaba_model_studio: 'alibaba-model-studio:beijing',
    qwen: 'alibaba-model-studio:beijing',
    open_ai: 'openai',
    azure_open_ai: 'azure-openai',
    deep_seek: 'deepseek',
  };
  return providerAliases[normalizedProvider] ?? normalizedProvider;
}

export function findSharedProviderCredential<T extends ProviderCredentialConfig>(
  configs: T[],
  provider: string,
  baseUrl?: string | null,
): T | null {
  const targetScope = providerCredentialScope(provider, baseUrl);
  const candidates = configs.filter(
    (config) =>
      config.apiKey.trim().length > 0 &&
      providerCredentialScope(config.provider, config.baseUrl) === targetScope,
  );
  return candidates.find((config) => config.isDefault) ?? candidates[0] ?? null;
}
