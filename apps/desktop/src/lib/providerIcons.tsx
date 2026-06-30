import type { CSSProperties } from 'react';

interface ProviderIconMeta {
  asset?: string;
  fallback: string;
  label: string;
  tone: string;
}

const PROVIDER_ICON_META: Record<string, ProviderIconMeta> = {
  openai: {
    asset: '/provider-icons/openai.svg',
    fallback: 'AI',
    label: 'OpenAI',
    tone: 'bg-text-primary/10 text-text-primary',
  },
  openrouter: {
    asset: '/provider-icons/openrouter.svg',
    fallback: 'OR',
    label: 'OpenRouter',
    tone: 'bg-slate-500/12 text-slate-300',
  },
  anthropic: {
    asset: '/provider-icons/anthropic.svg',
    fallback: 'A',
    label: 'Anthropic',
    tone: 'bg-amber-500/12 text-amber-500',
  },
  gemini: {
    asset: '/provider-icons/gemini.svg',
    fallback: 'G',
    label: 'Google Gemini',
    tone: 'bg-violet-500/12 text-violet-400',
  },
  google: {
    asset: '/provider-icons/google.svg',
    fallback: 'G',
    label: 'Google',
    tone: 'bg-blue-500/12 text-blue-500',
  },
  deepseek: {
    asset: '/provider-icons/deepseek.svg',
    fallback: 'DS',
    label: 'DeepSeek',
    tone: 'bg-blue-500/12 text-blue-400',
  },
  zhipu: {
    asset: '/provider-icons/zhipu.svg',
    fallback: 'GLM',
    label: 'Zhipu',
    tone: 'bg-indigo-500/12 text-indigo-400',
  },
  moonshot: {
    asset: '/provider-icons/moonshot.svg',
    fallback: 'K',
    label: 'Moonshot / Kimi',
    tone: 'bg-sky-500/12 text-sky-400',
  },
  qwen: {
    asset: '/provider-icons/qwen.svg',
    fallback: 'Q',
    label: 'Qwen',
    tone: 'bg-violet-500/12 text-violet-400',
  },
  doubao: {
    asset: '/provider-icons/doubao.svg',
    fallback: 'DB',
    label: 'Doubao',
    tone: 'bg-pink-500/12 text-pink-400',
  },
  yi: {
    asset: '/provider-icons/zeroone.svg',
    fallback: '01',
    label: '01.AI / Yi',
    tone: 'bg-rose-500/12 text-rose-400',
  },
  baichuan: {
    asset: '/provider-icons/baichuan.svg',
    fallback: 'BC',
    label: 'Baichuan',
    tone: 'bg-teal-500/12 text-teal-400',
  },
  ollama: {
    asset: '/provider-icons/ollama.svg',
    fallback: 'OL',
    label: 'Ollama',
    tone: 'bg-lime-500/12 text-lime-400',
  },
  lmstudio: {
    asset: '/provider-icons/lmstudio.svg',
    fallback: 'LM',
    label: 'LM Studio',
    tone: 'bg-fuchsia-500/12 text-fuchsia-400',
  },
  azureai: {
    asset: '/provider-icons/azureai.svg',
    fallback: 'AZ',
    label: 'Azure OpenAI',
    tone: 'bg-cyan-500/12 text-cyan-400',
  },
  mistral: {
    asset: '/provider-icons/mistral.svg',
    fallback: 'M',
    label: 'Mistral AI',
    tone: 'bg-orange-500/12 text-orange-400',
  },
  xai: {
    asset: '/provider-icons/xai.svg',
    fallback: 'xAI',
    label: 'xAI',
    tone: 'bg-text-primary/10 text-text-primary',
  },
  alibabacloud: {
    asset: '/provider-icons/alibabacloud.svg',
    fallback: 'Ali',
    label: 'Alibaba Cloud',
    tone: 'bg-orange-500/12 text-orange-400',
  },
  bytedance: {
    asset: '/provider-icons/bytedance.svg',
    fallback: 'BD',
    label: 'ByteDance',
    tone: 'bg-blue-500/12 text-blue-400',
  },
  custom: {
    fallback: 'AI',
    label: 'Custom provider',
    tone: 'bg-text-tertiary/12 text-text-secondary',
  },
};

const PROVIDER_TYPE_TO_ICON: Record<string, string> = {
  open_ai: 'openai',
  openrouter: 'openrouter',
  anthropic: 'anthropic',
  google: 'gemini',
  deep_seek: 'deepseek',
  zhipu: 'zhipu',
  moonshot: 'moonshot',
  qwen: 'qwen',
  doubao: 'doubao',
  yi: 'yi',
  baichuan: 'baichuan',
  ollama: 'ollama',
  lm_studio: 'lmstudio',
  azure_open_ai: 'azureai',
  custom: 'custom',
};

const PRESET_ID_TO_ICON: Record<string, string> = {
  openai: 'openai',
  openrouter: 'openrouter',
  anthropic: 'anthropic',
  google: 'gemini',
  deepseek: 'deepseek',
  xai: 'xai',
  mistral: 'mistral',
  ollama: 'ollama',
  lmstudio: 'lmstudio',
  zhipu: 'zhipu',
  moonshot: 'moonshot',
  qwen: 'qwen',
  doubao: 'doubao',
  yi: 'yi',
  baichuan: 'baichuan',
};

const BASE_URL_ICON_MATCHERS: Array<[RegExp, string]> = [
  [/openrouter\.ai/i, 'openrouter'],
  [/anthropic\.com/i, 'anthropic'],
  [/googleapis\.com|generativelanguage/i, 'gemini'],
  [/deepseek\.com/i, 'deepseek'],
  [/bigmodel\.cn/i, 'zhipu'],
  [/moonshot\.cn|moonshot\.ai|kimi/i, 'moonshot'],
  [/dashscope|aliyuncs|alibabacloud/i, 'qwen'],
  [/volces|volcengine|byte/i, 'doubao'],
  [/lingyiwanwu|01\.ai/i, 'yi'],
  [/baichuan-ai|baichuan/i, 'baichuan'],
  [/localhost:11434|ollama/i, 'ollama'],
  [/localhost:1234|lmstudio|lm-studio/i, 'lmstudio'],
  [/azure\.com|openai\.azure/i, 'azureai'],
  [/mistral\.ai/i, 'mistral'],
  [/x\.ai/i, 'xai'],
  [/openai\.com/i, 'openai'],
];

const LABEL_ICON_MATCHERS: Array<[RegExp, string]> = [
  [/openrouter/i, 'openrouter'],
  [/anthropic|claude/i, 'anthropic'],
  [/gemini|google/i, 'gemini'],
  [/deepseek/i, 'deepseek'],
  [/zhipu|glm/i, 'zhipu'],
  [/moonshot|kimi/i, 'moonshot'],
  [/qwen|dashscope|tongyi/i, 'qwen'],
  [/doubao|byte/i, 'doubao'],
  [/\b01\b|01\.ai|\byi\b/i, 'yi'],
  [/baichuan/i, 'baichuan'],
  [/ollama/i, 'ollama'],
  [/lm\s*studio/i, 'lmstudio'],
  [/azure/i, 'azureai'],
  [/mistral/i, 'mistral'],
  [/\bxai\b|\bx\.ai\b|grok/i, 'xai'],
  [/openai|gpt/i, 'openai'],
];

interface ResolveProviderIconInput {
  provider: string;
  providerId?: string | null;
  baseUrl?: string | null;
  label?: string | null;
}

interface ProviderIconProps extends ResolveProviderIconInput {
  className?: string;
  size?: 'xs' | 'sm' | 'md' | 'lg';
}

const sizeClasses = {
  xs: 'h-4 w-4',
  sm: 'h-6 w-6',
  md: 'h-8 w-8',
  lg: 'h-10 w-10',
};

const glyphSizeClasses = {
  xs: 'h-2.5 w-2.5',
  sm: 'h-3.5 w-3.5',
  md: 'h-[18px] w-[18px]',
  lg: 'h-[22px] w-[22px]',
};

const fallbackTextClasses = {
  xs: 'text-[7px]',
  sm: 'text-[9px]',
  md: 'text-[10px]',
  lg: 'text-xs',
};

function normalizeKey(value: string | null | undefined): string {
  return (value ?? '').trim().toLowerCase().replace(/[^a-z0-9]+/g, '');
}

export function resolveProviderIconMeta({
  provider,
  providerId,
  baseUrl,
  label,
}: ResolveProviderIconInput): ProviderIconMeta {
  const presetIcon = PRESET_ID_TO_ICON[normalizeKey(providerId)];
  if (presetIcon) {
    return PROVIDER_ICON_META[presetIcon];
  }

  const normalizedBaseUrl = (baseUrl ?? '').trim();
  if (normalizedBaseUrl) {
    const match = BASE_URL_ICON_MATCHERS.find(([pattern]) => pattern.test(normalizedBaseUrl));
    if (match) {
      return PROVIDER_ICON_META[match[1]];
    }
  }

  const labelText = (label ?? '').trim();
  if (labelText) {
    const match = LABEL_ICON_MATCHERS.find(([pattern]) => pattern.test(labelText));
    if (match) {
      return PROVIDER_ICON_META[match[1]];
    }
  }

  return PROVIDER_ICON_META[PROVIDER_TYPE_TO_ICON[provider] ?? 'custom'];
}

export function ProviderIcon({
  provider,
  providerId,
  baseUrl,
  label,
  className = '',
  size = 'md',
}: ProviderIconProps) {
  const meta = resolveProviderIconMeta({ provider, providerId, baseUrl, label });
  const maskStyle = meta.asset
    ? ({
        WebkitMask: `url("${meta.asset}") center / contain no-repeat`,
        mask: `url("${meta.asset}") center / contain no-repeat`,
        backgroundColor: 'currentColor',
      } satisfies CSSProperties)
    : undefined;

  return (
    <span
      className={`inline-flex shrink-0 items-center justify-center rounded-md ${sizeClasses[size]} ${meta.tone} ${className}`}
      title={meta.label}
      aria-hidden="true"
    >
      {meta.asset ? (
        <span className={glyphSizeClasses[size]} style={maskStyle} />
      ) : (
        <span className={`font-semibold leading-none tracking-normal ${fallbackTextClasses[size]}`}>
          {meta.fallback}
        </span>
      )}
    </span>
  );
}
