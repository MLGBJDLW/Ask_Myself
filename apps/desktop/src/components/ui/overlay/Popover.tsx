import * as Popover from '@radix-ui/react-popover';
import type { ComponentProps } from 'react';
import { useOverlayRoot } from './OverlayProvider';

export const NexaPopover = Popover.Root;
export const NexaPopoverAnchor = Popover.Anchor;
export const NexaPopoverTrigger = Popover.Trigger;
export const NexaPopoverClose = Popover.Close;

export function NexaPopoverContent({ className = '', ...props }: ComponentProps<typeof Popover.Content>) {
  const overlayRoot = useOverlayRoot();
  return (
    <Popover.Portal container={overlayRoot}>
      <Popover.Content
        {...props}
        collisionPadding={props.collisionPadding ?? 10}
        sideOffset={props.sideOffset ?? 6}
        className={`nexa-overlay-content pointer-events-auto ${className}`}
      />
    </Popover.Portal>
  );
}
