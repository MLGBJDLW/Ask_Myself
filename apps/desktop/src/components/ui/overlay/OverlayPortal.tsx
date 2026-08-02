import { createPortal } from 'react-dom';
import type { ReactNode } from 'react';
import { useOverlayRoot } from './OverlayProvider';

export function OverlayPortal({ children }: { children: ReactNode }) {
  const root = useOverlayRoot();
  if (typeof document === 'undefined') return null;
  return createPortal(children, root ?? document.body);
}
