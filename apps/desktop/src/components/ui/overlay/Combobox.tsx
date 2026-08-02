import { useMemo, useState, type ReactNode } from 'react';
import { Command } from 'cmdk';
import { Check, ChevronsUpDown } from 'lucide-react';
import {
  NexaPopover,
  NexaPopoverContent,
  NexaPopoverTrigger,
} from './Popover';

export interface NexaComboboxOption {
  value: string;
  label: string;
  keywords?: string[];
  disabled?: boolean;
  badge?: ReactNode;
}

export function NexaCombobox({
  ariaLabel,
  emptyLabel = 'No results',
  onValueChange,
  options,
  placeholder = 'Select…',
  searchPlaceholder = 'Search…',
  value,
}: {
  ariaLabel: string;
  emptyLabel?: string;
  onValueChange: (value: string) => void;
  options: NexaComboboxOption[];
  placeholder?: string;
  searchPlaceholder?: string;
  value?: string;
}) {
  const [open, setOpen] = useState(false);
  const selected = useMemo(() => options.find(option => option.value === value), [options, value]);

  return (
    <NexaPopover open={open} onOpenChange={setOpen}>
      <NexaPopoverTrigger asChild>
        <button type="button" className="nexa-select-trigger" aria-label={ariaLabel} aria-expanded={open}>
          <span className="truncate">{selected?.label ?? placeholder}</span>
          <ChevronsUpDown className="ml-2 h-3.5 w-3.5 shrink-0 text-text-tertiary" />
        </button>
      </NexaPopoverTrigger>
      <NexaPopoverContent className="min-w-[var(--radix-popover-trigger-width)] p-1">
        <Command loop>
          <Command.Input className="nexa-combobox-input" placeholder={searchPlaceholder} />
          <Command.List className="max-h-72 overflow-y-auto p-1">
            <Command.Empty className="px-2 py-6 text-center text-xs text-text-tertiary">{emptyLabel}</Command.Empty>
            {options.map(option => (
              <Command.Item
                key={option.value}
                value={[option.label, ...(option.keywords ?? [])].join(' ')}
                disabled={option.disabled}
                onSelect={() => {
                  onValueChange(option.value);
                  setOpen(false);
                }}
                className="nexa-overlay-item"
              >
                <Check className={`h-3.5 w-3.5 ${option.value === value ? 'opacity-100' : 'opacity-0'}`} />
                <span className="min-w-0 flex-1 truncate">{option.label}</span>
                {option.badge}
              </Command.Item>
            ))}
          </Command.List>
        </Command>
      </NexaPopoverContent>
    </NexaPopover>
  );
}
