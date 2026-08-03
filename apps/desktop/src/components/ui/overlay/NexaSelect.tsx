import {
  Children,
  Fragment,
  isValidElement,
  type ChangeEvent,
  type ComponentPropsWithoutRef,
  type ReactElement,
  type ReactNode,
  type SelectHTMLAttributes,
} from 'react';
import * as SelectPrimitive from '@radix-ui/react-select';
import * as DropdownMenu from '@radix-ui/react-dropdown-menu';
import { Check, ChevronDown, ChevronUp } from 'lucide-react';
import { useOverlayRoot } from './OverlayProvider';

type NativeSelectProps = Omit<
  SelectHTMLAttributes<HTMLSelectElement>,
  'multiple' | 'onChange' | 'size'
>;

export interface NexaSelectProps extends NativeSelectProps {
  multiple?: boolean;
  onChange?: (event: ChangeEvent<HTMLSelectElement>) => void;
}

interface SelectOption {
  value: string;
  label: ReactNode;
  disabled: boolean;
  group?: string;
}

const EMPTY_OPTION_VALUE = '__nexa_select_empty_value__';

function encodeOptionValue(value: string): string {
  return value === '' ? EMPTY_OPTION_VALUE : value;
}

function decodeOptionValue(value: string): string {
  return value === EMPTY_OPTION_VALUE ? '' : value;
}

function selectOptions(children: ReactNode, group?: string): SelectOption[] {
  return Children.toArray(children).flatMap(child => {
    if (!isValidElement(child)) return [];
    const element = child as ReactElement<{
      children?: ReactNode;
      disabled?: boolean;
      label?: string;
      value?: string | number;
    }>;
    if (element.type === Fragment) {
      return selectOptions(element.props.children, group);
    }
    if (element.type === 'optgroup') {
      return selectOptions(element.props.children, element.props.label);
    }
    if (element.type !== 'option') return [];
    const textValue = typeof element.props.children === 'string'
      || typeof element.props.children === 'number'
      ? String(element.props.children)
      : '';
    return [{
      value: String(element.props.value ?? textValue),
      label: element.props.children,
      disabled: Boolean(element.props.disabled),
      group,
    }];
  });
}

/** Radix-backed, theme-native replacement for the product's fixed-option selects. */
export function NexaSelect({
  children,
  className = '',
  defaultValue,
  disabled,
  id,
  multiple,
  name,
  onChange,
  value,
  ...props
}: NexaSelectProps) {
  const overlayRoot = useOverlayRoot();
  const options = selectOptions(children);
  const groups = [...new Set(options.map(option => option.group ?? ''))];
  const controlledValue = value == null ? undefined : encodeOptionValue(String(value));
  const initialValue = defaultValue == null ? undefined : encodeOptionValue(String(defaultValue));

  const emitChange = (nextValue: string, selectedValues: string[] = [nextValue]) => {
    if (!onChange) return;
    const decodedValue = decodeOptionValue(nextValue);
    const decodedSelectedValues = selectedValues.map(decodeOptionValue);
    const selectedOptions = decodedSelectedValues.map(optionValue => ({ value: optionValue }));
    const target = { id, name, selectedOptions, value: decodedValue } as unknown as HTMLSelectElement;
    onChange({ target, currentTarget: target } as ChangeEvent<HTMLSelectElement>);
  };

  if (multiple) {
    const selectedValues = Array.isArray(value)
      ? value.map(String)
      : value == null || value === ''
        ? []
        : [String(value)];
    const selectedLabels = options
      .filter(option => selectedValues.includes(option.value))
      .map(option => option.label)
      .filter((label): label is string | number => typeof label === 'string' || typeof label === 'number');
    return (
      <DropdownMenu.Root>
        <DropdownMenu.Trigger
          id={id}
          disabled={disabled}
          data-nexa-select-trigger="true"
          data-value={selectedValues.join(',')}
          aria-label={props['aria-label']}
          className={`nexa-select-trigger ${className}`}
        >
          <span className="truncate">
            {selectedLabels.length > 0 ? selectedLabels.join(', ') : `${selectedValues.length} selected`}
          </span>
          <ChevronDown className="ml-2 h-3.5 w-3.5 shrink-0 text-text-tertiary" />
        </DropdownMenu.Trigger>
        <DropdownMenu.Portal container={overlayRoot}>
          <DropdownMenu.Content
            className="nexa-overlay-content nexa-multi-select-content pointer-events-auto"
            sideOffset={6}
            collisionPadding={10}
          >
            <div className="nexa-multi-select-viewport">
              {options.map(option => {
                const checked = selectedValues.includes(option.value);
                return (
                  <DropdownMenu.CheckboxItem
                    key={option.value}
                    checked={checked}
                    disabled={option.disabled}
                    onSelect={event => event.preventDefault()}
                    onCheckedChange={() => {
                      const next = checked
                        ? selectedValues.filter(item => item !== option.value)
                        : [...selectedValues, option.value];
                      emitChange(next[0] ?? '', next);
                    }}
                    className="nexa-overlay-item nexa-select-item"
                  >
                    <DropdownMenu.ItemIndicator className="nexa-select-indicator">
                      <Check className="h-3.5 w-3.5" />
                    </DropdownMenu.ItemIndicator>
                    {option.label}
                  </DropdownMenu.CheckboxItem>
                );
              })}
            </div>
          </DropdownMenu.Content>
        </DropdownMenu.Portal>
      </DropdownMenu.Root>
    );
  }

  return (
    <SelectPrimitive.Root
      value={controlledValue}
      defaultValue={initialValue}
      disabled={disabled}
      name={name}
      required={props.required}
      onValueChange={emitChange}
    >
      <SelectPrimitive.Trigger
        {...(props as ComponentPropsWithoutRef<typeof SelectPrimitive.Trigger>)}
        id={id}
        data-nexa-select-trigger="true"
        data-value={value == null ? '' : String(value)}
        aria-label={props['aria-label']}
        aria-labelledby={props['aria-labelledby']}
        title={props.title}
        className={`nexa-select-trigger ${className}`}
      >
        <SelectPrimitive.Value className="min-w-0 flex-1 truncate text-left" />
        <SelectPrimitive.Icon className="ml-2 shrink-0 text-text-tertiary">
          <ChevronDown className="h-3.5 w-3.5" aria-hidden="true" />
        </SelectPrimitive.Icon>
      </SelectPrimitive.Trigger>
      <SelectPrimitive.Portal container={overlayRoot}>
        <SelectPrimitive.Content
          className="nexa-overlay-content nexa-select-content pointer-events-auto"
          position="popper"
          sideOffset={6}
          collisionPadding={10}
        >
          <SelectPrimitive.ScrollUpButton className="nexa-select-scroll-button">
            <ChevronUp className="h-3.5 w-3.5" />
          </SelectPrimitive.ScrollUpButton>
          <SelectPrimitive.Viewport className="nexa-select-viewport">
            {groups.map(group => (
              <SelectPrimitive.Group key={group || '__ungrouped'}>
                {group && (
                  <SelectPrimitive.Label className="nexa-select-label">
                    {group}
                  </SelectPrimitive.Label>
                )}
                {options.filter(option => (option.group ?? '') === group).map(option => (
                  <SelectPrimitive.Item
                    key={`${group}:${option.value}`}
                    value={encodeOptionValue(option.value)}
                    data-value={option.value}
                    disabled={option.disabled}
                    className="nexa-overlay-item nexa-select-item"
                  >
                    <SelectPrimitive.ItemIndicator className="nexa-select-indicator">
                      <Check className="h-3.5 w-3.5" />
                    </SelectPrimitive.ItemIndicator>
                    <SelectPrimitive.ItemText>
                      <span className="block min-w-0 truncate">{option.label}</span>
                    </SelectPrimitive.ItemText>
                  </SelectPrimitive.Item>
                ))}
              </SelectPrimitive.Group>
            ))}
          </SelectPrimitive.Viewport>
          <SelectPrimitive.ScrollDownButton className="nexa-select-scroll-button">
            <ChevronDown className="h-3.5 w-3.5" />
          </SelectPrimitive.ScrollDownButton>
        </SelectPrimitive.Content>
      </SelectPrimitive.Portal>
    </SelectPrimitive.Root>
  );
}
