import {
  BookOpen,
  BriefcaseBusiness,
  Code2,
  Database,
  Folder,
  FlaskConical,
  GraduationCap,
  Lightbulb,
  Palette,
  Rocket,
  ShieldCheck,
  Sparkles,
  type LucideIcon,
} from 'lucide-react';

export interface ProjectIconOption {
  id: string;
  label: string;
  icon: LucideIcon;
  tone: string;
}

export const PROJECT_ICON_OPTIONS: ProjectIconOption[] = [
  { id: 'folder', label: 'General', icon: Folder, tone: 'text-sky-500 bg-sky-500/10' },
  { id: 'book', label: 'Research', icon: BookOpen, tone: 'text-emerald-500 bg-emerald-500/10' },
  { id: 'briefcase', label: 'Work', icon: BriefcaseBusiness, tone: 'text-amber-500 bg-amber-500/10' },
  { id: 'code', label: 'Code', icon: Code2, tone: 'text-violet-500 bg-violet-500/10' },
  { id: 'database', label: 'Knowledge', icon: Database, tone: 'text-cyan-500 bg-cyan-500/10' },
  { id: 'rocket', label: 'Launch', icon: Rocket, tone: 'text-orange-500 bg-orange-500/10' },
  { id: 'sparkles', label: 'Creative', icon: Sparkles, tone: 'text-fuchsia-500 bg-fuchsia-500/10' },
  { id: 'palette', label: 'Design', icon: Palette, tone: 'text-rose-500 bg-rose-500/10' },
  { id: 'flask', label: 'Experiment', icon: FlaskConical, tone: 'text-lime-500 bg-lime-500/10' },
  { id: 'graduation', label: 'Learning', icon: GraduationCap, tone: 'text-blue-500 bg-blue-500/10' },
  { id: 'shield', label: 'Security', icon: ShieldCheck, tone: 'text-teal-500 bg-teal-500/10' },
  { id: 'idea', label: 'Ideas', icon: Lightbulb, tone: 'text-yellow-500 bg-yellow-500/10' },
];

const LEGACY_ICON_ALIASES: Record<string, string> = {
  '📁': 'folder',
  '📚': 'book',
  '💼': 'briefcase',
  '💻': 'code',
  '🚀': 'rocket',
  '✨': 'sparkles',
  '🎨': 'palette',
  '🔬': 'flask',
  '🎓': 'graduation',
  '🛡️': 'shield',
  '💡': 'idea',
};

export function normalizeProjectIconId(icon: string | null | undefined): string {
  const trimmed = icon?.trim() ?? '';
  return LEGACY_ICON_ALIASES[trimmed] ?? trimmed;
}

export function getProjectIconOption(icon: string | null | undefined): ProjectIconOption {
  const id = normalizeProjectIconId(icon);
  return PROJECT_ICON_OPTIONS.find((option) => option.id === id) ?? PROJECT_ICON_OPTIONS[0];
}

export function ProjectIcon({
  icon,
  className = '',
  size = 14,
}: {
  icon?: string | null;
  className?: string;
  size?: number;
}) {
  const option = getProjectIconOption(icon);
  const Icon = option.icon;

  return (
    <span
      className={`inline-flex shrink-0 items-center justify-center rounded-md ${option.tone} ${className}`}
      title={option.label}
      aria-hidden="true"
    >
      <Icon size={size} strokeWidth={1.8} />
    </span>
  );
}
