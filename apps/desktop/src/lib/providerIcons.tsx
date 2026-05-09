import {
  Atom,
  Bot,
  BrainCircuit,
  Cloud,
  Gem,
  Hexagon,
  Moon,
  Orbit,
  Server,
  Sparkles,
  Waves,
  Zap,
  type LucideIcon,
} from 'lucide-react';

const PROVIDER_ICON_META: Record<string, { icon: LucideIcon; tone: string; label: string }> = {
  open_ai: { icon: Atom, tone: 'bg-emerald-500/12 text-emerald-500', label: 'OpenAI' },
  anthropic: { icon: Sparkles, tone: 'bg-amber-500/12 text-amber-500', label: 'Anthropic' },
  google: { icon: Gem, tone: 'bg-sky-500/12 text-sky-500', label: 'Google' },
  deep_seek: { icon: Waves, tone: 'bg-cyan-500/12 text-cyan-500', label: 'DeepSeek' },
  zhipu: { icon: BrainCircuit, tone: 'bg-indigo-500/12 text-indigo-500', label: 'Zhipu' },
  moonshot: { icon: Moon, tone: 'bg-violet-500/12 text-violet-500', label: 'Moonshot' },
  qwen: { icon: Orbit, tone: 'bg-blue-500/12 text-blue-500', label: 'Qwen' },
  doubao: { icon: Zap, tone: 'bg-orange-500/12 text-orange-500', label: 'Doubao' },
  yi: { icon: Hexagon, tone: 'bg-rose-500/12 text-rose-500', label: 'Yi' },
  baichuan: { icon: Waves, tone: 'bg-teal-500/12 text-teal-500', label: 'Baichuan' },
  ollama: { icon: Server, tone: 'bg-lime-500/12 text-lime-500', label: 'Ollama' },
  lm_studio: { icon: Server, tone: 'bg-fuchsia-500/12 text-fuchsia-500', label: 'LM Studio' },
  azure_open_ai: { icon: Cloud, tone: 'bg-blue-600/12 text-blue-500', label: 'Azure OpenAI' },
  custom: { icon: Bot, tone: 'bg-text-tertiary/12 text-text-secondary', label: 'Custom provider' },
};

interface ProviderIconProps {
  provider: string;
  className?: string;
  size?: 'sm' | 'md' | 'lg';
}

const sizeClasses = {
  sm: 'h-6 w-6',
  md: 'h-8 w-8',
  lg: 'h-10 w-10',
};

const iconSizes = {
  sm: 13,
  md: 16,
  lg: 20,
};

export function ProviderIcon({ provider, className = '', size = 'md' }: ProviderIconProps) {
  const meta = PROVIDER_ICON_META[provider] ?? PROVIDER_ICON_META.custom;
  const Icon = meta.icon;

  return (
    <span
      className={`inline-flex shrink-0 items-center justify-center rounded-md ${sizeClasses[size]} ${meta.tone} ${className}`}
      title={meta.label}
      aria-hidden="true"
    >
      <Icon size={iconSizes[size]} strokeWidth={1.8} />
    </span>
  );
}
