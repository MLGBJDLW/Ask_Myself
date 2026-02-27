import { useState, useCallback, type KeyboardEvent, type ChangeEvent } from 'react';
import { useTranslation } from '../../i18n';

interface TagInputProps {
  value: string;
  onChange: (value: string) => void;
  presets?: { label: string; value: string }[];
  placeholder?: string;
  label?: string;
  showSelectAll?: boolean;
}

export function parseTags(value: string): string[] {
  const tags: string[] = [];
  let current = '';
  let braceDepth = 0;
  for (const ch of value) {
    if (ch === '{') braceDepth++;
    else if (ch === '}') braceDepth = Math.max(0, braceDepth - 1);
    if (ch === ',' && braceDepth === 0) {
      const trimmed = current.trim();
      if (trimmed) tags.push(trimmed);
      current = '';
    } else {
      current += ch;
    }
  }
  const trimmed = current.trim();
  if (trimmed) tags.push(trimmed);
  return tags;
}

function joinTags(tags: string[]): string {
  return tags.join(', ');
}

export function TagInput({
  value,
  onChange,
  presets,
  placeholder = 'Type and press Enter…',
  label,
  showSelectAll,
}: TagInputProps) {
  const { t } = useTranslation();
  const [inputValue, setInputValue] = useState('');
  const tags = parseTags(value);
  const tagSet = new Set(tags);

  const addTag = useCallback(
    (tag: string) => {
      const trimmed = tag.trim();
      if (!trimmed || tagSet.has(trimmed)) return;
      onChange(joinTags([...tags, trimmed]));
    },
    [tags, tagSet, onChange],
  );

  const removeTag = useCallback(
    (tag: string) => {
      onChange(joinTags(tags.filter((t) => t !== tag)));
    },
    [tags, onChange],
  );

  const togglePreset = useCallback(
    (preset: string) => {
      if (tagSet.has(preset)) {
        removeTag(preset);
      } else {
        addTag(preset);
      }
    },
    [tagSet, addTag, removeTag],
  );

  const handleKeyDown = (e: KeyboardEvent<HTMLInputElement>) => {
    if (e.key === 'Enter' || e.key === ',') {
      e.preventDefault();
      const raw = inputValue.replace(/,/g, '').trim();
      if (raw) {
        addTag(raw);
        setInputValue('');
      }
    } else if (e.key === 'Backspace' && !inputValue && tags.length > 0) {
      removeTag(tags[tags.length - 1]);
    }
  };

  const handleChange = (e: ChangeEvent<HTMLInputElement>) => {
    const v = e.target.value;
    // If the user pastes or types a comma, split and add all complete segments
    if (v.includes(',')) {
      const parts = v.split(',');
      const toAdd = parts.slice(0, -1);
      toAdd.forEach((p) => {
        const trimmed = p.trim();
        if (trimmed) addTag(trimmed);
      });
      setInputValue(parts[parts.length - 1]);
    } else {
      setInputValue(v);
    }
  };

  return (
    <div className="w-full space-y-1.5">
      {label && (
        <label className="block text-xs font-medium text-text-secondary">
          {label}
        </label>
      )}

      {/* Tag area + input */}
      <div
        className={`
          flex flex-wrap items-center gap-1.5 min-h-10 p-2
          bg-surface-1 border border-border rounded-md
          transition-all duration-fast ease-out
          focus-within:border-accent focus-within:ring-1 focus-within:ring-accent/30
        `}
      >
        {tags.map((tag) => (
          <span
            key={tag}
            className="inline-flex items-center gap-1 max-w-full px-2 py-0.5
              bg-surface-3 text-text-secondary text-[11px] font-medium rounded-full
              transition-colors duration-fast"
          >
            <span className="truncate">{tag}</span>
            <button
              type="button"
              onClick={() => removeTag(tag)}
              className="shrink-0 rounded-full p-0.5
                text-text-tertiary hover:text-text-primary hover:bg-surface-4
                transition-colors duration-fast cursor-pointer"
              aria-label={`Remove ${tag}`}
            >
              <svg
                xmlns="http://www.w3.org/2000/svg"
                viewBox="0 0 16 16"
                fill="currentColor"
                className="size-3"
              >
                <path d="M5.28 4.22a.75.75 0 0 0-1.06 1.06L6.94 8l-2.72 2.72a.75.75 0 1 0 1.06 1.06L8 9.06l2.72 2.72a.75.75 0 1 0 1.06-1.06L9.06 8l2.72-2.72a.75.75 0 0 0-1.06-1.06L8 6.94 5.28 4.22Z" />
              </svg>
            </button>
          </span>
        ))}

        <input
          type="text"
          value={inputValue}
          onChange={handleChange}
          onKeyDown={handleKeyDown}
          placeholder={tags.length === 0 ? placeholder : ''}
          className="flex-1 min-w-[80px] bg-transparent text-sm text-text-primary
            placeholder:text-text-tertiary outline-none border-none p-0"
        />
      </div>

      {/* Preset buttons */}
      {presets && presets.length > 0 && (
        <div className="flex flex-wrap gap-1.5">
          {showSelectAll && (() => {
            const allActive = presets.every((p) => tagSet.has(p.value));
            return (
              <button
                type="button"
                onClick={() => {
                  if (allActive) {
                    // Clear all preset values
                    const presetValues = new Set(presets.map((p) => p.value));
                    onChange(joinTags(tags.filter((t) => !presetValues.has(t))));
                  } else {
                    // Add all missing presets
                    const missing = presets.filter((p) => !tagSet.has(p.value)).map((p) => p.value);
                    onChange(joinTags([...tags, ...missing]));
                  }
                }}
                className={`
                  inline-flex items-center px-2 py-0.5 text-[11px] font-semibold
                  rounded-full cursor-pointer select-none
                  transition-colors duration-fast ease-out
                  ${
                    allActive
                      ? 'bg-red-500/15 text-red-400 border border-red-500/40 hover:bg-red-500/25'
                      : 'bg-accent/10 text-accent border border-accent/30 hover:bg-accent/20'
                  }
                `}
              >
                {allActive ? t('common.clearAll') : t('common.selectAll')}
              </button>
            );
          })()}
          {presets.map((preset) => {
            const active = tagSet.has(preset.value);
            return (
              <button
                key={preset.value}
                type="button"
                onClick={() => togglePreset(preset.value)}
                className={`
                  inline-flex items-center px-2 py-0.5 text-[11px] font-medium
                  rounded-full cursor-pointer select-none
                  transition-colors duration-fast ease-out
                  ${
                    active
                      ? 'bg-accent/20 text-accent-hover border border-accent/40'
                      : 'bg-surface-2 text-text-tertiary border border-border hover:text-text-secondary hover:border-border-hover'
                  }
                `}
              >
                {preset.label}
              </button>
            );
          })}
        </div>
      )}
    </div>
  );
}
