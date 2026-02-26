import { useState, useEffect, useRef, useCallback } from 'react';
import { motion, AnimatePresence } from 'framer-motion';
import { ScrollText, Check, X } from 'lucide-react';
import { useTranslation } from '../../i18n';
import * as api from '../../lib/api';
import { toast } from 'sonner';

interface SystemPromptEditorProps {
  conversationId: string;
  /** Current system prompt from the Conversation object */
  systemPrompt: string;
  /** Called after a successful save with the new prompt value */
  onSaved?: (newPrompt: string) => void;
}

export function SystemPromptEditor({
  conversationId,
  systemPrompt,
  onSaved,
}: SystemPromptEditorProps) {
  const { t } = useTranslation();
  const [open, setOpen] = useState(false);
  const [draft, setDraft] = useState(systemPrompt);
  const [saving, setSaving] = useState(false);
  const ref = useRef<HTMLDivElement>(null);

  // Sync draft when prop changes (e.g. switching conversations)
  useEffect(() => {
    setDraft(systemPrompt);
    setOpen(false);
  }, [systemPrompt, conversationId]);

  // Close on outside click
  useEffect(() => {
    if (!open) return;
    const handler = (e: MouseEvent) => {
      if (ref.current && !ref.current.contains(e.target as Node)) {
        setOpen(false);
        setDraft(systemPrompt); // revert unsaved changes
      }
    };
    document.addEventListener('mousedown', handler);
    return () => document.removeEventListener('mousedown', handler);
  }, [open, systemPrompt]);

  const hasCustomPrompt = systemPrompt.trim().length > 0;
  const isDirty = draft !== systemPrompt;

  const handleSave = useCallback(async () => {
    setSaving(true);
    try {
      await api.updateConversationSystemPrompt(conversationId, draft.trim());
      onSaved?.(draft.trim());
      setOpen(false);
    } catch (e) {
      toast.error(String(e));
    } finally {
      setSaving(false);
    }
  }, [conversationId, draft, onSaved]);

  const handleClear = useCallback(async () => {
    setSaving(true);
    try {
      await api.updateConversationSystemPrompt(conversationId, '');
      setDraft('');
      onSaved?.('');
      setOpen(false);
    } catch (e) {
      toast.error(String(e));
    } finally {
      setSaving(false);
    }
  }, [conversationId, onSaved]);

  return (
    <div ref={ref} className="relative inline-block">
      {/* Trigger button */}
      <button
        type="button"
        onClick={() => setOpen((v) => !v)}
        aria-expanded={open}
        aria-haspopup="dialog"
        className="
          flex items-center gap-1.5 px-2 py-1 rounded-md text-xs
          hover:bg-surface-2 transition-colors duration-fast
          border border-transparent hover:border-border
        "
        title={hasCustomPrompt ? t('chat.customPrompt') : t('chat.defaultPrompt')}
      >
        <ScrollText
          size={14}
          className={hasCustomPrompt ? 'text-accent' : 'text-text-tertiary'}
        />
        <span className={hasCustomPrompt ? 'text-accent' : 'text-text-tertiary'}>
          {t('chat.systemPrompt')}
        </span>
        {hasCustomPrompt && (
          <span className="w-1.5 h-1.5 rounded-full bg-accent shrink-0" />
        )}
      </button>

      {/* Dropdown editor */}
      <AnimatePresence>
        {open && (
          <motion.div
            initial={{ opacity: 0, y: -4, scale: 0.97 }}
            animate={{ opacity: 1, y: 0, scale: 1 }}
            exit={{ opacity: 0, y: -4, scale: 0.97 }}
            transition={{ duration: 0.15 }}
            className="
              absolute left-0 top-full mt-1 z-50
              w-80 rounded-lg border border-border bg-surface-1 shadow-lg
            "
          >
            <div className="px-3 py-2 border-b border-border">
              <p className="text-xs font-medium text-text-primary">
                {t('chat.editSystemPrompt')}
              </p>
            </div>

            <div className="p-3">
              <textarea
                value={draft}
                onChange={(e) => setDraft(e.target.value)}
                placeholder={t('chat.defaultPrompt')}
                rows={4}
                className="
                  w-full rounded-md border border-border bg-surface-2
                  px-2.5 py-2 text-xs text-text-primary
                  placeholder:text-text-tertiary
                  focus:outline-none focus:ring-1 focus:ring-accent
                  resize-y min-h-[80px] max-h-[200px]
                "
              />
            </div>

            <div className="px-3 py-2 border-t border-border flex items-center justify-between gap-2">
              {hasCustomPrompt && (
                <button
                  type="button"
                  onClick={handleClear}
                  disabled={saving}
                  className="
                    flex items-center gap-1 px-2 py-1 rounded text-xs
                    text-text-tertiary hover:text-text-primary
                    hover:bg-surface-2 transition-colors
                  "
                >
                  <X size={12} />
                  <span>{t('common.clear')}</span>
                </button>
              )}
              <div className="flex-1" />
              <button
                type="button"
                onClick={handleSave}
                disabled={saving || !isDirty}
                className="
                  flex items-center gap-1 px-3 py-1 rounded text-xs
                  bg-accent text-on-accent
                  hover:bg-accent/90 transition-colors
                  disabled:opacity-50 disabled:cursor-not-allowed
                "
              >
                <Check size={12} />
                <span>{t('chat.savePrompt')}</span>
              </button>
            </div>
          </motion.div>
        )}
      </AnimatePresence>
    </div>
  );
}
