import { useEffect, useRef, useState, type ReactNode } from 'react';
import { AnimatePresence, motion, useReducedMotion } from 'framer-motion';
import { useTranslation } from '../../i18n';

interface DiffStatsTickerProps {
  additions: number;
  deletions: number;
  filesChanged?: number;
  replacements?: number | null;
  compact?: boolean;
  live?: boolean;
  showFiles?: boolean;
  showReplacements?: boolean;
  testIdPrefix?: string;
}

function normalizedCount(value: number | null | undefined): number {
  return Number.isFinite(value) ? Math.max(0, Math.round(value ?? 0)) : 0;
}

function RollingCount({
  value,
  className,
}: {
  value: number;
  className?: string;
}) {
  const shouldReduceMotion = useReducedMotion();
  const previousRef = useRef(value);
  const [direction, setDirection] = useState(1);
  const digits = String(value).split('');

  useEffect(() => {
    const previous = previousRef.current;
    setDirection(value >= previous ? 1 : -1);
    previousRef.current = value;
  }, [value]);

  if (shouldReduceMotion) {
    return (
      <span className={`inline-block text-right tabular-nums ${className ?? ''}`}>
        {value}
      </span>
    );
  }

  return (
    <span className={`inline-flex justify-end tabular-nums ${className ?? ''}`}>
      {digits.map((digit, index) => (
        <span
          key={`${digits.length}-${index}`}
          className="relative inline-flex h-[1.15em] w-[1ch] overflow-hidden align-[-0.12em]"
        >
          <AnimatePresence initial={false} mode="popLayout">
            <motion.span
              key={`${digits.length}-${index}-${digit}`}
              initial={{ y: direction > 0 ? '82%' : '-82%', opacity: 0 }}
              animate={{ y: 0, opacity: 1 }}
              exit={{ y: direction > 0 ? '-82%' : '82%', opacity: 0 }}
              transition={{ duration: 0.18, ease: [0.22, 1, 0.36, 1] }}
              className="absolute inset-0 text-center"
            >
              {digit}
            </motion.span>
          </AnimatePresence>
        </span>
      ))}
    </span>
  );
}

function SignedRollingCount({ value, sign }: { value: number; sign: '+' | '-' }) {
  return (
    <span className="inline-flex min-w-[2.6ch] justify-end tabular-nums">
      <span className="shrink-0">{sign}</span>
      <RollingCount value={value} />
    </span>
  );
}

function StatPill({
  tone,
  kind,
  valueLabel,
  testId,
  compact,
  live,
  children,
}: {
  tone: 'add' | 'delete' | 'neutral';
  kind: 'additions' | 'deletions' | 'files' | 'replacements';
  valueLabel: string;
  testId?: string;
  compact?: boolean;
  live?: boolean;
  children: ReactNode;
}) {
  const size = compact ? 'h-5 px-1.5 text-[10px]' : 'h-6 px-2 text-[11px]';
  const toneClass =
    tone === 'add'
      ? 'border-success/25 bg-success/10 text-success'
      : tone === 'delete'
        ? 'border-danger/25 bg-danger/10 text-danger'
        : 'border-border/60 bg-surface-0/70 text-text-tertiary';

  return (
    <span
      data-testid={testId}
      data-diff-stat-kind={kind}
      data-value={valueLabel}
      className={`${size} inline-flex items-center gap-1 rounded-md border font-mono leading-none shadow-[inset_0_1px_0_rgba(255,255,255,0.04)] ${toneClass} ${
        live ? 'motion-safe:animate-pulse' : ''
      }`}
    >
      {children}
    </span>
  );
}

export function DiffStatsTicker({
  additions,
  deletions,
  filesChanged = 1,
  replacements = null,
  compact = false,
  live = false,
  showFiles = true,
  showReplacements = true,
  testIdPrefix,
}: DiffStatsTickerProps) {
  const { t } = useTranslation();
  const additionsValue = normalizedCount(additions);
  const deletionsValue = normalizedCount(deletions);
  const filesValue = normalizedCount(filesChanged);
  const replacementsValue = normalizedCount(replacements);
  const statTestId = (kind: string) => (testIdPrefix ? `${testIdPrefix}-${kind}` : undefined);

  return (
    <div
      data-testid={testIdPrefix ? `${testIdPrefix}-diff-stats` : undefined}
      className="inline-flex shrink-0 items-center gap-1 tabular-nums"
      aria-label={`+${additionsValue} -${deletionsValue}`}
      data-live-diff-stats={live ? 'true' : 'false'}
    >
      <StatPill
        tone="add"
        kind="additions"
        valueLabel={`+${additionsValue}`}
        testId={statTestId('additions')}
        compact={compact}
        live={live}
      >
        <SignedRollingCount value={additionsValue} sign="+" />
      </StatPill>
      <StatPill
        tone="delete"
        kind="deletions"
        valueLabel={`-${deletionsValue}`}
        testId={statTestId('deletions')}
        compact={compact}
        live={live}
      >
        <SignedRollingCount value={deletionsValue} sign="-" />
      </StatPill>
      {showFiles && filesValue > 1 && (
        <StatPill
          tone="neutral"
          kind="files"
          valueLabel={String(filesValue)}
          testId={statTestId('files')}
          compact={compact}
        >
          <RollingCount value={filesValue} />
          <span className="font-sans">{t('chat.diffFiles')}</span>
        </StatPill>
      )}
      {showReplacements && replacementsValue > 0 && (
        <StatPill
          tone="neutral"
          kind="replacements"
          valueLabel={String(replacementsValue)}
          testId={statTestId('replacements')}
          compact={compact}
        >
          <RollingCount value={replacementsValue} />
          <span className="hidden font-sans sm:inline">{t('chat.diffReplacements')}</span>
        </StatPill>
      )}
    </div>
  );
}
