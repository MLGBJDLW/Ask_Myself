import { normalizeModelEndpointUrl } from './modelCatalog';

export interface ProviderCredentialConfig {
  provider: string;
  baseUrl?: string | null;
  apiKey: string;
  name: string;
  isDefault?: boolean;
}

function normalizedUrl(value: string | null | undefined): string {
  return normalizeModelEndpointUrl(value);
}

function endpointUrl(value: string | null | undefined): URL | null {
  const normalized = normalizedUrl(value);
  if (!normalized) return null;
  try {
    return new URL(normalized);
  } catch {
    return null;
  }
}

const TRUSTED_CREDENTIAL_ENDPOINTS: Readonly<Record<string, string>> = {
  'https://dashscope.aliyuncs.com/compatible-mode/v1': 'alibaba-model-studio:beijing',
  'https://dashscope.aliyuncs.com/api-ws/v1': 'alibaba-model-studio:beijing',
  'https://dashscope.aliyuncs.com/api/v1/services/aigc/multimodal-generation/generation': 'alibaba-model-studio:beijing',
  'https://dashscope.aliyuncs.com/api/v1/services/audio/tts': 'alibaba-model-studio:beijing',
  'https://dashscope-intl.aliyuncs.com/compatible-mode/v1': 'alibaba-model-studio:singapore',
  'https://dashscope-intl.aliyuncs.com/api/v1/services/aigc/multimodal-generation/generation': 'alibaba-model-studio:singapore',
  'https://dashscope-us.aliyuncs.com/compatible-mode/v1': 'alibaba-model-studio:virginia',
  'https://api.openai.com/v1': 'openai',
  'https://api.anthropic.com': 'anthropic',
  'https://api.anthropic.com/v1': 'anthropic',
  'https://generativelanguage.googleapis.com/v1beta': 'google',
  'https://api.groq.com/openai/v1': 'groq',
  'https://api.mistral.ai/v1': 'mistral',
  'https://api.minimax.io/v1': 'minimax',
  'https://api.meta.ai/v1': 'meta-model-api',
  'https://api.jina.ai/v1': 'jina',
  'https://api.siliconflow.cn/v1': 'siliconflow',
  'https://open.bigmodel.cn/api/paas/v4': 'zhipu:china',
  'https://api.z.ai/api/paas/v4': 'zhipu:international',
};

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

  if (normalizedBaseUrl.includes('token-plan.') || normalizedBaseUrl.includes('coding.dashscope.')) {
    return `endpoint:${normalizedBaseUrl}`;
  }

  const endpoint = endpointUrl(baseUrl);
  const hasTrustedShape = endpoint?.protocol === 'https:'
    && !endpoint.port
    && !endpoint.username
    && !endpoint.password
    && !endpoint.search
    && !endpoint.hash;
  const trustedScope = hasTrustedShape
    ? TRUSTED_CREDENTIAL_ENDPOINTS[normalizedBaseUrl]
    : undefined;
  if (trustedScope) return trustedScope;

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
    zhipu: 'zhipu:china',
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
