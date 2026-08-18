import { type CSSProperties } from 'react';
import { useTranslation } from '../i18n';

interface LogoProps {
  size?: number;
  className?: string;
  decorative?: boolean;
}

export function Logo({ size = 32, className, decorative = false }: LogoProps) {
  const { t } = useTranslation();
  const compact = size <= 16;
  const style = { width: size, height: size } as CSSProperties;

  return (
    <svg
      viewBox="0 0 32 32"
      width={size}
      height={size}
      className={`nexa-logo-mark ${className ?? ''}`.trim()}
      style={style}
      role={decorative ? undefined : 'img'}
      aria-hidden={decorative || undefined}
      aria-label={decorative ? undefined : t('app.name')}
      data-optical-size={compact ? 'compact' : 'full'}
    >
      <path
        className="nexa-logo-orbit"
        d="M5.8 16C8.3 9.2 12.4 5.8 16 5.8S23.7 9.2 26.2 16C23.7 22.8 19.6 26.2 16 26.2S8.3 22.8 5.8 16Z"
        fill="none"
        stroke="var(--nexa-logo-muted, currentColor)"
        strokeWidth="1.15"
        strokeLinecap="round"
        strokeLinejoin="round"
      />
      <path
        className="nexa-logo-monogram"
        d="M9.2 23.4V8.6l13.6 14.8V8.6"
        fill="none"
        stroke="currentColor"
        strokeWidth={compact ? 4.4 : 3.8}
        strokeLinecap="round"
        strokeLinejoin="round"
      />
      <circle
        className="nexa-logo-cutout"
        cx="16"
        cy="16"
        r="4.15"
        fill="var(--nexa-logo-cutout, var(--color-surface-1))"
      />
      <circle
        cx="16"
        cy="16"
        r={compact ? 2.15 : 2.55}
        fill="none"
        stroke="var(--nexa-logo-accent, currentColor)"
        strokeWidth="1.35"
      />
      {!compact && (
        <path
          className="nexa-logo-detail"
          d="M17.85 14.25c.9.75 1.2 1.85.82 2.95"
          fill="none"
          stroke="var(--nexa-logo-accent, currentColor)"
          strokeWidth="1.1"
          strokeLinecap="round"
        />
      )}
      <circle cx="16" cy="16" r="0.58" fill="var(--nexa-logo-accent, currentColor)" />
      {!compact && (
        <g className="nexa-logo-detail" fill="currentColor">
          <circle cx="9.2" cy="8.6" r="1" />
          <circle cx="22.8" cy="23.4" r="1" />
        </g>
      )}
    </svg>
  );
}
