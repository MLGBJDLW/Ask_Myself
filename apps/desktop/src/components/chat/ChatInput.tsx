import { useState, useRef, useCallback, useEffect } from 'react';
import { motion } from 'framer-motion';
import { Send, Square, Paperclip, X, Scissors, AlertTriangle, Gauge, Brain, Clock3, Plus } from 'lucide-react';
import { useTranslation } from '../../i18n';
import type { ImageAttachment } from '../../types/conversation';
import { CheckpointMenu } from './CheckpointMenu';

interface TokenUsage {
  promptTokens: number;
  totalTokens: number;
  contextWindow: number;
  completionTokens: number;
  thinkingTokens: number;
  isEstimated: boolean;
}

interface ChatInputProps {
  onSend: (message: string, attachments?: ImageAttachment[]) => void;
  onStop: () => void;
  isStreaming: boolean;
  disabled: boolean;
  tokenUsage?: TokenUsage | null;
  onCompact?: () => void;
  onStartNewChat?: () => void;
  finishReason?: string | null;
  contextOverflow?: boolean;
  rateLimited?: boolean;
  conversationId?: string;
  onRestoreCheckpoint?: () => void;
  prefillText?: string;
}

function formatTokens(n: number): string {
  if (n >= 1_000_000) return `${(n / 1_000_000).toFixed(1)}M`;
  if (n >= 1_000) return `${(n / 1_000).toFixed(n >= 10_000 ? 0 : 1)}K`;
  return String(n);
}

export function ChatInput({
  onSend,
  onStop,
  isStreaming,
  disabled,
  tokenUsage,
  onCompact,
  onStartNewChat,
  finishReason,
  contextOverflow,
  rateLimited,
  conversationId,
  onRestoreCheckpoint,
  prefillText,
}: ChatInputProps) {
  const { t } = useTranslation();
  const [value, setValue] = useState('');
  const [attachments, setAttachments] = useState<ImageAttachment[]>([]);
  const [isDragging, setIsDragging] = useState(false);
  const textareaRef = useRef<HTMLTextAreaElement>(null);
  const fileInputRef = useRef<HTMLInputElement>(null);
  const dragCounterRef = useRef(0);

  // Accept prefilled text from outside (e.g. suggestion cards)
  useEffect(() => {
    if (prefillText != null && prefillText !== '') {
      setValue(prefillText);
      setTimeout(() => textareaRef.current?.focus(), 0);
    }
  }, [prefillText]);

  const usage = tokenUsage && tokenUsage.contextWindow > 0 ? tokenUsage : null;
  const usagePercent = usage ? Math.min(100, (usage.promptTokens / usage.contextWindow) * 100) : 0;
  const usagePercentRounded = Math.round(usagePercent);
  const usageNearLimit = usagePercent >= 80;
  const usageCritical = contextOverflow || usagePercent >= 95;
  const canCompactOverflow = Boolean(onCompact);
  const canStartNewChatFromOverflow = Boolean(onStartNewChat);
  const contextOverflowHintKey = canCompactOverflow && canStartNewChatFromOverflow
    ? 'chat.contextOverflowActionsHint'
    : canCompactOverflow
      ? 'chat.contextOverflowHint'
      : canStartNewChatFromOverflow
        ? 'chat.contextOverflowNewChatHint'
        : null;
  const usageHint = usageNearLimit
    ? (usageCritical
      ? t('chat.contextNearlyFull', { percent: usagePercentRounded })
      : t('chat.contextFillingUp', { percent: usagePercentRounded }))
    : null;
  const usageBarColor = usageCritical ? 'var(--color-danger)' : usageNearLimit ? 'var(--color-warning)' : usagePercent >= 60 ? '#eab308' : 'var(--color-info)';

  // Auto-resize textarea
  const adjustHeight = useCallback(() => {
    const el = textareaRef.current;
    if (!el) return;
    el.style.height = 'auto';
    const lineHeight = 22;
    const maxHeight = lineHeight * 6 + 16;
    el.style.height = `${Math.min(el.scrollHeight, maxHeight)}px`;
  }, []);

  useEffect(() => {
    adjustHeight();
  }, [value, adjustHeight]);

  const handleSend = useCallback(() => {
    const trimmed = value.trim();
    if (!trimmed && attachments.length === 0) return;
    onSend(trimmed || t('chat.imageMessage'), attachments.length > 0 ? attachments : undefined);
    setValue('');
    setAttachments([]);
    setTimeout(() => {
      if (textareaRef.current) {
        textareaRef.current.style.height = 'auto';
      }
    }, 0);
  }, [value, attachments, onSend, t]);

  const handleKeyDown = useCallback(
    (e: React.KeyboardEvent) => {
      if (e.key === 'Enter' && !e.shiftKey) {
        e.preventDefault();
        if (!isStreaming && !disabled) {
          handleSend();
        }
      }
    },
    [handleSend, isStreaming, disabled],
  );

  const handleFileSelect = useCallback(async (e: React.ChangeEvent<HTMLInputElement>) => {
    const files = e.target.files;
    if (!files) return;
    for (const file of Array.from(files)) {
      try {
        const reader = new FileReader();
        const result = await new Promise<string>((resolve, reject) => {
          reader.onload = () => resolve(reader.result as string);
          reader.onerror = reject;
          reader.readAsDataURL(file);
        });
        const match = result.match(/^data:([^;]+);base64,(.+)$/);
        if (!match) continue;
        const [, mediaType, base64Data] = match;
        if (!['image/jpeg', 'image/png', 'image/gif', 'image/webp'].includes(mediaType)) continue;
        setAttachments((prev) => [...prev, {
          base64Data,
          mediaType,
          originalName: file.name,
        }]);
      } catch {
        // Silently skip files that fail to read
      }
    }
    e.target.value = '';
  }, []);

  const removeAttachment = useCallback((index: number) => {
    setAttachments((prev) => prev.filter((_, i) => i !== index));
  }, []);

  const handleDragOver = useCallback((e: React.DragEvent) => {
    e.preventDefault();
    e.stopPropagation();
  }, []);

  const handleDragEnter = useCallback((e: React.DragEvent) => {
    e.preventDefault();
    e.stopPropagation();
    dragCounterRef.current += 1;
    if (e.dataTransfer.types.includes('Files')) {
      setIsDragging(true);
    }
  }, []);

  const handleDragLeave = useCallback((e: React.DragEvent) => {
    e.preventDefault();
    e.stopPropagation();
    dragCounterRef.current -= 1;
    if (dragCounterRef.current <= 0) {
      dragCounterRef.current = 0;
      setIsDragging(false);
    }
  }, []);

  const handleDrop = useCallback(async (e: React.DragEvent) => {
    e.preventDefault();
    e.stopPropagation();
    dragCounterRef.current = 0;
    setIsDragging(false);
    const files = e.dataTransfer.files;
    if (!files) return;
    for (const file of Array.from(files)) {
      if (!file.type.startsWith('image/')) continue;
      try {
        const reader = new FileReader();
        const result = await new Promise<string>((resolve, reject) => {
          reader.onload = () => resolve(reader.result as string);
          reader.onerror = reject;
          reader.readAsDataURL(file);
        });
        const match = result.match(/^data:([^;]+);base64,(.+)$/);
        if (!match) continue;
        const [, mediaType, base64Data] = match;
        setAttachments((prev) => [...prev, {
          base64Data,
          mediaType,
          originalName: file.name,
        }]);
      } catch {
        // Silently skip
      }
    }
  }, []);

  return (
    <div
      data-testid="chat-input"
      className={`relative border-t border-border bg-surface-1 px-4 py-3 transition-colors ${
        isDragging ? 'ring-2 ring-accent/50 bg-accent-subtle' : ''
      }`}
      onDragOver={handleDragOver}
      onDragEnter={handleDragEnter}
      onDragLeave={handleDragLeave}
      onDrop={handleDrop}
    >
      {isDragging && (
        <div className="absolute inset-0 z-10 flex items-center justify-center rounded-lg border-2 border-dashed border-accent bg-accent-subtle/50 pointer-events-none">
          <span className="text-sm font-medium text-accent">{t('chat.dragDropHint')}</span>
        </div>
      )}

      {finishReason === 'length' && !isStreaming && (
        <div className="mb-2 flex items-center gap-2 rounded-lg border border-yellow-500/20 bg-yellow-500/10 px-2.5 py-1.5 text-xs text-yellow-600">
          <AlertTriangle className="h-3.5 w-3.5" />
          <span>{t('chat.truncated')}</span>
        </div>
      )}
      {finishReason === 'contentfilter' && !isStreaming && (
        <div className="mb-2 flex items-center gap-2 rounded-lg border border-red-500/20 bg-red-500/10 px-2.5 py-1.5 text-xs text-red-400">
          <AlertTriangle className="h-3.5 w-3.5" />
          <span>{t('chat.contentFiltered')}</span>
        </div>
      )}
      {contextOverflow && !isStreaming && (
        <div className="mb-2 rounded-lg border border-orange-500/25 bg-orange-500/10 px-2.5 py-1.5 text-xs text-orange-400">
          <div className="flex items-start gap-2">
            <AlertTriangle className="mt-0.5 h-3.5 w-3.5 shrink-0" />
            <div className="min-w-0 flex-1">
              <div className="flex flex-wrap items-center gap-2">
                <span className="min-w-0 flex-1">{t('chat.contextOverflow')}</span>
                {(canCompactOverflow || canStartNewChatFromOverflow) && (
                  <div className="flex flex-wrap gap-1.5">
                    {canCompactOverflow && (
                      <button
                        type="button"
                        onClick={onCompact}
                        className="inline-flex items-center gap-1 rounded-md bg-orange-500/20 px-2 py-0.5 text-[11px] font-medium text-orange-200 transition-colors cursor-pointer hover:bg-orange-500/30"
                      >
                        <Scissors size={12} />
                        <span>{t('chat.compact')}</span>
                      </button>
                    )}
                    {canStartNewChatFromOverflow && (
                      <button
                        type="button"
                        onClick={onStartNewChat}
                        className="inline-flex items-center gap-1 rounded-md bg-orange-500/20 px-2 py-0.5 text-[11px] font-medium text-orange-200 transition-colors cursor-pointer hover:bg-orange-500/30"
                      >
                        <Plus size={12} />
                        <span>{t('chat.startNewChat')}</span>
                      </button>
                    )}
                  </div>
                )}
              </div>
              {contextOverflowHintKey && (
                <p className="mt-1 text-[11px] text-orange-300">
                  {t(contextOverflowHintKey)}
                </p>
              )}
            </div>
          </div>
        </div>
      )}
      {rateLimited && !isStreaming && (
        <div className="mb-2 flex items-center gap-2 rounded-lg border border-yellow-500/20 bg-yellow-500/10 px-2.5 py-1.5 text-xs text-yellow-600">
          <Clock3 className="h-3.5 w-3.5" />
          <span>{t('chat.rateLimited')}</span>
        </div>
      )}

      {usage && (
        <div
          data-testid="context-usage-card"
          className={`mb-2 rounded-xl border px-3 py-2 ${
          usageCritical
            ? 'border-red-500/25 bg-red-500/10'
            : usageNearLimit
              ? 'border-yellow-500/20 bg-yellow-500/10'
              : 'border-border/70 bg-surface-0/70'
        }`}
        >
          <div className="flex items-center gap-2">
            <Gauge className="h-3.5 w-3.5 shrink-0 text-text-tertiary" />
            <span data-testid="context-usage-percent" className="shrink-0 text-[11px] tabular-nums text-text-secondary">
              {t('chat.tokenUsagePercent', { percent: usagePercentRounded })}
            </span>
            <div className="h-2 flex-1 overflow-hidden rounded-full bg-surface-3/80">
              <div
                className="h-full rounded-full transition-all duration-500"
                style={{ width: `${Math.max(usagePercent, 0.8)}%`, backgroundColor: usageBarColor }}
              />
            </div>
            {usage.isEstimated && (
              <span className="shrink-0 rounded bg-surface-3 px-1 py-0.5 text-[10px] text-text-tertiary" title="Estimated">
                ~
              </span>
            )}
          </div>

          <div className="mt-1.5 flex flex-wrap items-center gap-x-3 gap-y-1 text-[11px]">
            <span className="tabular-nums text-text-secondary">
              {t('chat.tokenUsage', {
                used: formatTokens(usage.promptTokens),
                total: formatTokens(usage.contextWindow),
              })}
            </span>
            <span className="tabular-nums text-text-tertiary">
              {t('chat.tokenIO', {
                input: formatTokens(usage.promptTokens),
                output: formatTokens(usage.completionTokens),
              })}
            </span>
            {usage.thinkingTokens > 0 && (
              <span
                className="inline-flex items-center gap-1 tabular-nums text-accent/80"
                title={t('chat.thinkingTokens', { tokens: String(usage.thinkingTokens) })}
              >
                <Brain className="h-3 w-3" />
                {formatTokens(usage.thinkingTokens)}
              </span>
            )}
            {onCompact && (usagePercent >= 60 || contextOverflow) && (
              <button
                onClick={onCompact}
                className="inline-flex items-center gap-1 rounded-md px-1.5 py-0.5 text-text-tertiary hover:bg-surface-3 hover:text-text-secondary transition-colors cursor-pointer"
                title={t('chat.compact')}
              >
                <Scissors size={12} />
                <span>{t('chat.compact')}</span>
              </button>
            )}
            {conversationId && onRestoreCheckpoint && (
              <CheckpointMenu conversationId={conversationId} onRestore={onRestoreCheckpoint} />
            )}
          </div>

          {usageHint && !contextOverflow && (
            <div className="mt-1.5 flex flex-wrap items-center gap-2 text-xs">
              <span className={usageCritical ? 'text-red-400' : 'text-yellow-600'}>
                {usageHint}
              </span>
              {onStartNewChat && (
                <button
                  type="button"
                  onClick={onStartNewChat}
                  className={`inline-flex items-center gap-1 rounded-md px-2 py-0.5 text-[11px] font-medium transition-colors cursor-pointer ${
                    usageCritical
                      ? 'bg-red-500/15 text-red-300 hover:bg-red-500/25'
                      : 'bg-yellow-500/15 text-yellow-700 hover:bg-yellow-500/25'
                  }`}
                >
                  <Plus size={12} />
                  {t('chat.startNewChat')}
                </button>
              )}
            </div>
          )}
        </div>
      )}

      {attachments.length > 0 && (
        <div className="flex flex-wrap gap-2 pb-2">
          {attachments.map((att, i) => (
            <div key={i} className="relative group">
              <img
                src={`data:${att.mediaType};base64,${att.base64Data}`}
                alt={att.originalName}
                className="h-16 w-16 rounded-md border border-border object-cover"
              />
              <button
                onClick={() => removeAttachment(i)}
                className="absolute -right-1.5 -top-1.5 flex h-4 w-4 items-center justify-center rounded-full bg-danger text-[10px] leading-none text-white opacity-0 transition-opacity cursor-pointer group-hover:opacity-100"
                aria-label={t('chat.removeAttachment')}
              >
                <X className="h-3 w-3" />
              </button>
              <span className="absolute bottom-0 left-0 right-0 truncate rounded-b-md bg-black/50 px-1 text-[9px] text-white">
                {att.originalName}
              </span>
            </div>
          ))}
        </div>
      )}

      <div className="flex items-end gap-2">
        <motion.button
          whileTap={{ scale: 0.95 }}
          onClick={() => fileInputRef.current?.click()}
          disabled={disabled || isStreaming}
          className="flex h-10 w-10 shrink-0 items-center justify-center rounded-lg text-text-tertiary transition-colors duration-fast ease-out cursor-pointer hover:bg-surface-2 hover:text-text-secondary disabled:pointer-events-none disabled:opacity-40"
          aria-label={t('chat.attachImage')}
        >
          <Paperclip className="h-4 w-4" />
        </motion.button>
        <input
          ref={fileInputRef}
          type="file"
          accept="image/jpeg,image/png,image/gif,image/webp"
          multiple
          hidden
          onChange={handleFileSelect}
        />

        <textarea
          data-testid="chat-input-textarea"
          ref={textareaRef}
          value={value}
          onChange={(e) => setValue(e.target.value)}
          onKeyDown={handleKeyDown}
          placeholder={t('chat.placeholder')}
          disabled={disabled}
          rows={1}
          className="flex-1 resize-none rounded-lg border border-border bg-surface-0 px-3.5 py-2.5 text-sm text-text-primary placeholder:text-text-tertiary outline-none transition-all duration-fast ease-out hover:border-border-hover focus:border-accent focus:ring-1 focus:ring-accent/30 disabled:pointer-events-none disabled:opacity-40"
        />

        {isStreaming ? (
          <motion.button
            whileTap={{ scale: 0.95 }}
            onClick={onStop}
            className="flex h-10 w-10 shrink-0 items-center justify-center rounded-lg bg-danger/10 text-danger transition-colors duration-fast ease-out cursor-pointer hover:bg-danger/20"
            aria-label={t('chat.stop')}
          >
            <Square className="h-4 w-4" />
          </motion.button>
        ) : (
          <motion.button
            whileTap={{ scale: 0.95 }}
            onClick={handleSend}
            disabled={disabled || (!value.trim() && attachments.length === 0)}
            data-testid="chat-send"
            className="flex h-10 w-10 shrink-0 items-center justify-center rounded-lg bg-accent text-white transition-colors duration-fast ease-out cursor-pointer hover:bg-accent-hover disabled:pointer-events-none disabled:opacity-40"
            aria-label={t('chat.send')}
          >
            <Send className="h-4 w-4" />
          </motion.button>
        )}
      </div>
    </div>
  );
}
