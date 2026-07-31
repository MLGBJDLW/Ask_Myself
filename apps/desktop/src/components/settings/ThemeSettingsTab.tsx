import { Palette } from 'lucide-react';

import { useTranslation } from '../../i18n';
import { ThemeSwitcher } from '../ui/ThemeSwitcher';
import { Section } from './SettingsSection';
import { ThemeStudio } from './ThemeStudio';

export function ThemeSettingsTab() {
  const { t } = useTranslation();

  return (
    <Section
      icon={<Palette size={20} />}
      title={t('themeStudio.title')}
      description={t('themeStudio.description')}
      delay={0.03}
    >
      <div className="space-y-5">
        <div>
          <p className="mb-2 text-sm font-medium text-text-primary">
            {t('settings.appearance.theme')}
          </p>
          <p className="mb-3 text-xs text-text-tertiary">
            {t('settings.appearance.theme.description')}
          </p>
          <ThemeSwitcher />
        </div>
        <ThemeStudio />
      </div>
    </Section>
  );
}
