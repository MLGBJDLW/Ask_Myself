import * as ContextMenu from '@radix-ui/react-context-menu';
import type { ComponentProps } from 'react';
import { useOverlayRoot } from './OverlayProvider';

export const NexaContextMenu = ContextMenu.Root;
export const NexaContextMenuTrigger = ContextMenu.Trigger;

export function NexaContextMenuContent({ className = '', ...props }: ComponentProps<typeof ContextMenu.Content>) {
  const overlayRoot = useOverlayRoot();
  return (
    <ContextMenu.Portal container={overlayRoot}>
      <ContextMenu.Content
        {...props}
        collisionPadding={props.collisionPadding ?? 10}
        className={`nexa-overlay-content pointer-events-auto p-1 ${className}`}
      />
    </ContextMenu.Portal>
  );
}

export function NexaContextMenuItem({ className = '', ...props }: ComponentProps<typeof ContextMenu.Item>) {
  return <ContextMenu.Item {...props} className={`nexa-overlay-item ${className}`} />;
}
