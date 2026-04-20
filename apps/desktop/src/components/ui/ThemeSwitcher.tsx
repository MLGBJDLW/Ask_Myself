import { Moon, Sun, Star } from 'lucide-react';
import { motion } from 'framer-motion';
import type { LucideProps } from 'lucide-react';
import { useTheme } from '../../lib/ThemeProvider';
import { THEMES, type ThemeId } from '../../lib/theme';
import { useTranslation } from '../../i18n';
import type { TranslationKeys } from '../../i18n';

const ICON_MAP: Record<ThemeId, React.ComponentType<LucideProps>> = {
  dark: Moon,
  light: Sun,
  midnight: Star,
};

interface ThemeSwitcherProps {
  /** Show labels next to icons (default: true) */
  showLabels?: boolean;
}

export function ThemeSwitcher({ showLabels = true }: ThemeSwitcherProps) {
  const { theme, setTheme } = useTheme();
  const { t } = useTranslation();

  const themeLabel = (id: ThemeId) => t(`settings.appearance.theme.${id}` as keyof TranslationKeys);

  return (
    <div className="flex gap-1 rounded-lg bg-surface-2 p-1">
      {THEMES.map((themeOption) => {
        const Icon = ICON_MAP[themeOption.id];
        const isActive = theme === themeOption.id;
        const label = themeLabel(themeOption.id);
        return (
          <button
            key={themeOption.id}
            onClick={() => setTheme(themeOption.id)}
            className={`relative flex items-center justify-center gap-1.5 rounded-md px-3 py-1.5 text-xs font-medium
              transition-colors duration-150 cursor-pointer
              ${isActive ? 'text-text-inverse' : 'text-text-secondary hover:text-text-primary'}`}
            aria-label={label}
          >
            {isActive && (
              <motion.span
                layoutId="theme-indicator"
                className="absolute inset-0 rounded-md bg-accent"
                transition={{ type: 'spring', stiffness: 400, damping: 30 }}
              />
            )}
            <span className="relative flex items-center gap-1.5">
              <Icon size={14} />
              {showLabels && <span>{label}</span>}
            </span>
          </button>
        );
      })}
    </div>
  );
}

export default ThemeSwitcher;
