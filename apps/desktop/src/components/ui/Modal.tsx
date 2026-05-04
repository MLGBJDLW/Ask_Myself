import { useEffect, useRef, type ReactNode } from 'react';
import { motion, AnimatePresence, useReducedMotion } from 'framer-motion';
import { X } from 'lucide-react';
import { useTranslation } from '../../i18n';
import { getSoftDropdownMotion, INSTANT_TRANSITION } from '../../lib/uiMotion';

const FOCUSABLE_SELECTOR = [
  'button:not([disabled])',
  '[href]',
  'input:not([disabled])',
  'select:not([disabled])',
  'textarea:not([disabled])',
  '[tabindex]:not([tabindex="-1"]):not([disabled])',
].join(', ');

function isFocusable(element: HTMLElement) {
  return !element.hasAttribute('aria-hidden') && element.getClientRects().length > 0;
}

function getFocusableElements(container: HTMLDivElement | null) {
  if (!container) return [];

  return Array.from(container.querySelectorAll<HTMLElement>(FOCUSABLE_SELECTOR)).filter(isFocusable);
}

interface ModalProps {
  open: boolean;
  onClose: () => void;
  title: string;
  children: ReactNode;
  footer?: ReactNode;
}

export function Modal({ open, onClose, title, children, footer }: ModalProps) {
  const { t } = useTranslation();
  const shouldReduceMotion = useReducedMotion();
  const contentRef = useRef<HTMLDivElement>(null);
  const restoreFocusRef = useRef<HTMLElement | null>(null);

  useEffect(() => {
    if (!open) return;

    restoreFocusRef.current = document.activeElement instanceof HTMLElement ? document.activeElement : null;

    const focusInitialElement = () => {
      const container = contentRef.current;
      if (!container) return;

      if (document.activeElement instanceof HTMLElement && container.contains(document.activeElement)) {
        return;
      }

      const focusableElements = getFocusableElements(container);
      const fallbackTarget = focusableElements[0] ?? container;
      fallbackTarget.focus();
    };

    const animationFrame = window.requestAnimationFrame(focusInitialElement);

    const handleKeyDown = (e: KeyboardEvent) => {
      if (e.key === 'Escape') {
        onClose();
        return;
      }

      if (e.key !== 'Tab') return;

      const container = contentRef.current;
      if (!container) return;

      const focusableElements = getFocusableElements(container);
      if (focusableElements.length === 0) {
        e.preventDefault();
        container.focus();
        return;
      }

      const firstElement = focusableElements[0];
      const lastElement = focusableElements[focusableElements.length - 1];
      const activeElement = document.activeElement instanceof HTMLElement ? document.activeElement : null;
      const isFocusInside = activeElement ? container.contains(activeElement) : false;

      if (e.shiftKey) {
        if (!isFocusInside || activeElement === firstElement || activeElement === container) {
          e.preventDefault();
          lastElement.focus();
        }
        return;
      }

      if (!isFocusInside || activeElement === lastElement) {
        e.preventDefault();
        firstElement.focus();
      }
    };

    document.addEventListener('keydown', handleKeyDown);

    return () => {
      window.cancelAnimationFrame(animationFrame);
      document.removeEventListener('keydown', handleKeyDown);

      const restoreTarget = restoreFocusRef.current;
      restoreFocusRef.current = null;

      if (restoreTarget?.isConnected) {
        window.requestAnimationFrame(() => {
          restoreTarget.focus();
        });
      }
    };
  }, [open, onClose]);

  return (
    <AnimatePresence>
      {open && (
        <div className="fixed inset-0 z-50 flex items-center justify-center">
          <motion.div
            initial={shouldReduceMotion ? false : { opacity: 0 }}
            animate={{ opacity: 1 }}
            exit={{ opacity: 0 }}
            transition={shouldReduceMotion ? INSTANT_TRANSITION : { duration: 0.15 }}
            className="absolute inset-0 bg-black/60 backdrop-blur-sm"
            onClick={onClose}
            aria-hidden="true"
          />
          <motion.div
            ref={contentRef}
            {...getSoftDropdownMotion(!!shouldReduceMotion, 8)}
            role="dialog"
            aria-modal="true"
            aria-label={title}
            tabIndex={-1}
            className="relative z-10 w-full max-w-md bg-surface-2 border border-border rounded-lg shadow-lg overflow-hidden"
          >
            <div className="flex items-center justify-between px-5 py-4 border-b border-border">
              <h2 className="text-sm font-semibold text-text-primary">{title}</h2>
              <button
                onClick={onClose}
                className="p-1 rounded-md text-text-tertiary hover:text-text-primary hover:bg-surface-3 transition-colors"
                aria-label={t('common.close')}
              >
                <X size={16} />
              </button>
            </div>
            <div className="px-5 py-4">
              {children}
            </div>
            {footer && (
              <div className="flex items-center justify-end gap-2 px-5 py-3 border-t border-border bg-surface-1">
                {footer}
              </div>
            )}
          </motion.div>
        </div>
      )}
    </AnimatePresence>
  );
}
