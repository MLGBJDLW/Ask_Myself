import { useEffect, useState, type ReactNode } from 'react';
import { useReducedMotion } from 'framer-motion';

type CollapsibleMotionMode = 'compact' | 'heavy';

interface CollapsibleMotionProps {
  open: boolean;
  children: ReactNode;
  mode?: CollapsibleMotionMode;
  className?: string;
  contentClassName?: string;
  testId?: string;
}

/**
 * Shared disclosure seam for chat UI. Compact content uses a grid-track
 * transition; heavy content also clips and translates while retaining its own
 * scroll container. No measured or spring-driven `height: auto` is involved.
 */
export function CollapsibleMotion({
  open,
  children,
  mode = 'compact',
  className = '',
  contentClassName = '',
  testId,
}: CollapsibleMotionProps) {
  const shouldReduceMotion = useReducedMotion();
  const [present, setPresent] = useState(open);

  useEffect(() => {
    if (open) {
      setPresent(true);
      return undefined;
    }

    if (shouldReduceMotion) {
      setPresent(false);
      return undefined;
    }

    const timeout = window.setTimeout(() => setPresent(false), mode === 'heavy' ? 220 : 160);
    return () => window.clearTimeout(timeout);
  }, [mode, open, shouldReduceMotion]);

  if (!present && !open) return null;

  return (
    <div
      data-testid={testId}
      data-open={open ? 'true' : 'false'}
      data-motion={mode}
      aria-hidden={!open}
      className={`nexa-collapsible-motion nexa-collapsible-motion--${mode} ${className}`}
    >
      <div className={`nexa-collapsible-motion__content ${contentClassName}`}>{children}</div>
    </div>
  );
}

export function FadeMotion({
  visible,
  children,
  className = '',
}: {
  visible: boolean;
  children: ReactNode;
  className?: string;
}) {
  return (
    <div data-visible={visible ? 'true' : 'false'} className={`nexa-fade-motion ${className}`}>
      {children}
    </div>
  );
}

export function OverlayMotion({ children, className = '' }: { children: ReactNode; className?: string }) {
  return <div className={`nexa-overlay-motion ${className}`}>{children}</div>;
}
