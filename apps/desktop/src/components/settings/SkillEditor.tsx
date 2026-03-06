import { useEffect, useRef, useState } from 'react';
import { Save, X } from 'lucide-react';
import { Button } from '../ui/Button';
import { Input } from '../ui/Input';
import { useTranslation } from '../../i18n';
import type { Skill, SaveSkillInput } from '../../types/extensions';

interface SkillEditorProps {
  skill?: Skill;
  onSave: (input: SaveSkillInput) => void;
  onCancel: () => void;
  onDirtyChange?: (dirty: boolean) => void;
}

function estimateTokens(text: string): number {
  if (!text) return 0;
  let tokens = 0;
  for (let i = 0; i < text.length; i++) {
    tokens += text.charCodeAt(i) > 0x2fff ? 1.5 : 0.25;
  }
  return Math.ceil(tokens);
}

export function SkillEditor({ skill, onSave, onCancel, onDirtyChange }: SkillEditorProps) {
  const { t } = useTranslation();
  const [name, setName] = useState(skill?.name ?? '');
  const [content, setContent] = useState(skill?.content ?? '');
  const initialDraftRef = useRef({
    name: skill?.name ?? '',
    content: skill?.content ?? '',
  });

  useEffect(() => {
    if (!onDirtyChange) return;

    const dirty = name !== initialDraftRef.current.name || content !== initialDraftRef.current.content;
    onDirtyChange(dirty);
  }, [content, name, onDirtyChange]);

  useEffect(() => {
    if (!onDirtyChange) return;

    return () => {
      onDirtyChange(false);
    };
  }, [onDirtyChange]);

  const handleSubmit = () => {
    if (!name.trim() || !content.trim()) return;
    onSave({
      id: skill?.id ?? null,
      name: name.trim(),
      content: content.trim(),
      enabled: skill?.enabled ?? true,
    });
  };

  const tokenCount = estimateTokens(content);

  return (
    <div className="space-y-4">
      <div className="space-y-2">
        <label className="text-sm font-medium text-text-primary">{t('settings.skillName')}</label>
        <Input
          value={name}
          onChange={(e) => setName(e.target.value)}
          placeholder={t('settings.skillName')}
        />
      </div>

      <div className="space-y-2">
        <label className="text-sm font-medium text-text-primary">{t('settings.skillContent')}</label>
        <textarea
          value={content}
          onChange={(e) => setContent(e.target.value)}
          placeholder={t('settings.skillContentPlaceholder')}
          rows={6}
          className="w-full rounded-md border border-border bg-surface-2 px-3 py-2 text-sm text-text-primary font-mono placeholder:text-text-tertiary focus:border-accent focus:outline-none focus:ring-1 focus:ring-accent resize-y"
        />
        <p className="text-xs text-text-tertiary">
          {t('settings.skillTokenEstimate', { count: String(tokenCount) })}
        </p>
      </div>

      <div className="flex items-center gap-2 pt-2 border-t border-border">
        <Button
          variant="primary"
          size="sm"
          icon={<Save size={14} />}
          onClick={handleSubmit}
          disabled={!name.trim() || !content.trim()}
        >
          {t('common.save')}
        </Button>
        <Button variant="ghost" size="sm" icon={<X size={14} />} onClick={onCancel}>
          {t('common.cancel')}
        </Button>
      </div>
    </div>
  );
}
