import { useState, type ReactNode } from 'react';
import { motion, AnimatePresence, useReducedMotion } from 'framer-motion';
import { ChevronDown } from 'lucide-react';
import { useTranslation } from '../../i18n';
import { getSoftCollapseMotion, INSTANT_TRANSITION } from '../../lib/uiMotion';

interface SectionProps {
  icon: ReactNode;
  title: string;
  children: ReactNode;
  delay?: number;
  description?: string;
  summary?: ReactNode;
  collapsible?: boolean;
  defaultOpen?: boolean;
}

export function Section({
  icon,
  title,
  children,
  delay = 0,
  description,
  summary,
  collapsible = false,
  defaultOpen = true,
}: SectionProps) {
  const { t } = useTranslation();
  const shouldReduceMotion = useReducedMotion();
  const [open, setOpen] = useState(defaultOpen);

  const header = (
    <div className="flex min-w-0 flex-1 items-start gap-2.5">
      <span className="mt-0.5 shrink-0 text-accent">{icon}</span>
      <div className="min-w-0">
        <h2 className="text-base font-semibold text-text-primary">{title}</h2>
        {description && (
          <p className="mt-1 text-xs leading-relaxed text-text-tertiary">{description}</p>
        )}
      </div>
    </div>
  );

  return (
    <motion.section
      initial={shouldReduceMotion ? false : { opacity: 0, y: 10 }}
      animate={{ opacity: 1, y: 0 }}
      transition={shouldReduceMotion ? INSTANT_TRANSITION : { duration: 0.24, delay, ease: [0.16, 1, 0.3, 1] }}
      className="overflow-hidden rounded-xl border border-border bg-surface-1"
    >
      {collapsible ? (
        <button
          type="button"
          onClick={() => setOpen((value) => !value)}
          aria-expanded={open}
          aria-label={open ? t('common.collapse') : t('common.expand')}
          className="flex w-full items-start justify-between gap-3 px-6 py-5 text-left transition-colors hover:bg-surface-2/60"
        >
          {header}
          <div className="flex shrink-0 items-center gap-2">
            {summary}
            <ChevronDown
              size={16}
              className={`mt-0.5 text-text-tertiary transition-transform ${open ? 'rotate-180' : ''}`}
            />
          </div>
        </button>
      ) : (
        <div className="px-6 pt-6">
          <div className="mb-5 flex items-center gap-2.5">{header}</div>
        </div>
      )}

      {collapsible ? (
        <AnimatePresence initial={false}>
          {open && (
            <motion.div
              {...getSoftCollapseMotion(!!shouldReduceMotion)}
              className="overflow-hidden"
            >
              <div className="border-t border-border px-6 py-5">
                {children}
              </div>
            </motion.div>
          )}
        </AnimatePresence>
      ) : (
        <div className="px-6 pb-6">
          {children}
        </div>
      )}
    </motion.section>
  );
}

export function StatCard({ label, value }: { label: string; value: number | string }) {
  return (
    <div className="rounded-lg bg-surface-2 px-4 py-3">
      <p className="text-xs text-text-tertiary">{label}</p>
      <p className="mt-1 text-xl font-bold text-text-primary">{value}</p>
    </div>
  );
}

interface CollapsiblePanelProps {
  title: string;
  description?: string;
  children: ReactNode;
  defaultOpen?: boolean;
  summary?: ReactNode;
  open?: boolean;
  onOpenChange?: (open: boolean) => void;
}

export function CollapsiblePanel({
  title,
  description,
  children,
  defaultOpen = false,
  summary,
  open: controlledOpen,
  onOpenChange,
}: CollapsiblePanelProps) {
  const { t } = useTranslation();
  const shouldReduceMotion = useReducedMotion();
  const [internalOpen, setInternalOpen] = useState(defaultOpen);
  const open = controlledOpen ?? internalOpen;
  const toggleOpen = () => {
    const next = !open;
    if (controlledOpen === undefined) {
      setInternalOpen(next);
    }
    onOpenChange?.(next);
  };

  return (
    <div className="overflow-hidden rounded-lg border border-border bg-surface-1">
      <button
        type="button"
        onClick={toggleOpen}
        aria-expanded={open}
        aria-label={open ? t('common.collapse') : t('common.expand')}
        className="flex w-full items-start justify-between gap-3 px-4 py-3 text-left transition-colors hover:bg-surface-2/70"
      >
        <div className="min-w-0">
          <h4 className="text-sm font-medium text-text-primary">{title}</h4>
          {description && (
            <p className="mt-1 text-xs leading-relaxed text-text-tertiary">{description}</p>
          )}
        </div>
        <div className="flex shrink-0 items-center gap-2">
          {summary}
          <ChevronDown
            size={16}
            className={`mt-0.5 text-text-tertiary transition-transform ${open ? 'rotate-180' : ''}`}
          />
        </div>
      </button>
      <AnimatePresence initial={false}>
        {open && (
          <motion.div
            {...getSoftCollapseMotion(!!shouldReduceMotion)}
            className="overflow-hidden"
          >
            <div className="border-t border-border px-4 py-4">
              {children}
            </div>
          </motion.div>
        )}
      </AnimatePresence>
    </div>
  );
}
