import { useTranslation } from '../i18n';

interface LogoProps {
  size?: number;
  className?: string;
}

export function Logo({ size = 32, className }: LogoProps) {
  const { t } = useTranslation();
  const src = size <= 32 ? '/logo-small.svg' : '/logo.svg';
  return (
    <img
      src={src}
      alt={t('app.name')}
      width={size}
      height={size}
      className={className}
    />
  );
}
