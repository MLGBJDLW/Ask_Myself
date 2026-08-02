import * as DropdownMenu from '@radix-ui/react-dropdown-menu';
import type { ComponentProps } from 'react';
import { useOverlayRoot } from './OverlayProvider';

export const NexaMenu = DropdownMenu.Root;
export const NexaMenuTrigger = DropdownMenu.Trigger;
export const NexaMenuSeparator = DropdownMenu.Separator;

export function NexaMenuContent({ className = '', ...props }: ComponentProps<typeof DropdownMenu.Content>) {
  const overlayRoot = useOverlayRoot();
  return (
    <DropdownMenu.Portal container={overlayRoot}>
      <DropdownMenu.Content
        {...props}
        collisionPadding={props.collisionPadding ?? 10}
        sideOffset={props.sideOffset ?? 6}
        className={`nexa-overlay-content pointer-events-auto p-1 ${className}`}
      />
    </DropdownMenu.Portal>
  );
}

export function NexaMenuItem({ className = '', ...props }: ComponentProps<typeof DropdownMenu.Item>) {
  return <DropdownMenu.Item {...props} className={`nexa-overlay-item ${className}`} />;
}
