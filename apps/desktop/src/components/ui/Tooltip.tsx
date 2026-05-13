import { useState, useRef, useCallback, useEffect, useLayoutEffect, type ReactNode } from 'react';
import { createPortal } from 'react-dom';
import { motion, AnimatePresence } from 'framer-motion';

interface TooltipProps {
  content: string;
  children: ReactNode;
  side?: 'top' | 'bottom';
  delay?: number;
}

export function Tooltip({ content, children, side = 'top', delay = 300 }: TooltipProps) {
  const [show, setShow] = useState(false);
  const [position, setPosition] = useState<{ left: number; top: number } | null>(null);
  const triggerRef = useRef<HTMLDivElement>(null);
  const timerRef = useRef<ReturnType<typeof setTimeout>>();

  const updatePosition = useCallback(() => {
    const rect = triggerRef.current?.getBoundingClientRect();
    if (!rect || typeof window === 'undefined') return;
    const padding = 12;
    const left = Math.min(
      Math.max(rect.left + rect.width / 2, padding),
      window.innerWidth - padding,
    );
    setPosition({
      left,
      top: side === 'top' ? rect.top - 8 : rect.bottom + 8,
    });
  }, [side]);

  const handleEnter = useCallback(() => {
    timerRef.current = setTimeout(() => {
      updatePosition();
      setShow(true);
    }, delay);
  }, [delay, updatePosition]);

  const handleLeave = useCallback(() => {
    clearTimeout(timerRef.current);
    setShow(false);
  }, []);

  useLayoutEffect(() => {
    if (show) updatePosition();
  }, [content, show, updatePosition]);

  useEffect(() => {
    if (!show) return undefined;
    const handleMove = () => updatePosition();
    window.addEventListener('scroll', handleMove, true);
    window.addEventListener('resize', handleMove);
    return () => {
      window.removeEventListener('scroll', handleMove, true);
      window.removeEventListener('resize', handleMove);
    };
  }, [show, updatePosition]);

  const tooltip = (
    <AnimatePresence>
      {show && position && (
        <motion.div
          initial={{ opacity: 0, y: side === 'top' ? 4 : -4 }}
          animate={{ opacity: 1, y: 0 }}
          exit={{ opacity: 0, y: side === 'top' ? 4 : -4 }}
          transition={{ duration: 0.15 }}
          className="
            fixed z-[9999] max-w-[min(32rem,calc(100vw-1.5rem))]
            rounded-md border border-border/70 bg-surface-4 px-2.5 py-1.5
            text-xs font-medium text-text-primary shadow-lg shadow-black/15
            pointer-events-none whitespace-normal break-all
          "
          style={{
            left: position.left,
            top: position.top,
            transform: side === 'top' ? 'translate(-50%, -100%)' : 'translate(-50%, 0)',
          }}
        >
          {content}
        </motion.div>
      )}
    </AnimatePresence>
  );

  return (
    <div
      ref={triggerRef}
      className="inline-flex"
      onMouseEnter={handleEnter}
      onMouseLeave={handleLeave}
    >
      {children}
      {typeof document !== 'undefined' ? createPortal(tooltip, document.body) : null}
    </div>
  );
}
