import { useState, useRef, useCallback, useEffect } from 'react';
import { motion } from 'framer-motion';
import { Send, Square, Paperclip, X } from 'lucide-react';
import { useTranslation } from '../../i18n';
import type { ImageAttachment } from '../../types/conversation';

/* ------------------------------------------------------------------ */
/*  Types                                                              */
/* ------------------------------------------------------------------ */

interface TokenUsage {
  promptTokens: number;
  totalTokens: number;
  contextWindow: number;
}

interface ChatInputProps {
  onSend: (message: string, attachments?: ImageAttachment[]) => void;
  onStop: () => void;
  isStreaming: boolean;
  disabled: boolean;
  tokenUsage?: TokenUsage | null;
}

/* ------------------------------------------------------------------ */
/*  Component                                                          */
/* ------------------------------------------------------------------ */

function formatTokens(n: number): string {
  if (n >= 1_000_000) return `${(n / 1_000_000).toFixed(1)}M`;
  if (n >= 1_000) return `${(n / 1_000).toFixed(n >= 10_000 ? 0 : 1)}K`;
  return String(n);
}

export function ChatInput({ onSend, onStop, isStreaming, disabled, tokenUsage }: ChatInputProps) {
  const { t } = useTranslation();
  const [value, setValue] = useState('');
  const [attachments, setAttachments] = useState<ImageAttachment[]>([]);
  const textareaRef = useRef<HTMLTextAreaElement>(null);
  const fileInputRef = useRef<HTMLInputElement>(null);

  // Auto-resize textarea
  const adjustHeight = useCallback(() => {
    const el = textareaRef.current;
    if (!el) return;
    el.style.height = 'auto';
    const lineHeight = 22;
    const maxHeight = lineHeight * 6 + 16; // ~6 lines + padding
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
    // Reset height after send
    setTimeout(() => {
      if (textareaRef.current) {
        textareaRef.current.style.height = 'auto';
      }
    }, 0);
  }, [value, attachments, onSend]);

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
        // Use Tauri's file path if available (from drag/drop or file dialog)
        // For input[type=file], we read as base64 in the frontend
        const reader = new FileReader();
        const result = await new Promise<string>((resolve, reject) => {
          reader.onload = () => resolve(reader.result as string);
          reader.onerror = reject;
          reader.readAsDataURL(file);
        });
        // Extract base64 and media type from data URL
        const match = result.match(/^data:([^;]+);base64,(.+)$/);
        if (!match) continue;
        const [, mediaType, base64Data] = match;
        if (!['image/jpeg', 'image/png', 'image/gif', 'image/webp'].includes(mediaType)) continue;
        setAttachments(prev => [...prev, {
          base64Data,
          mediaType,
          originalName: file.name,
        }]);
      } catch {
        // Silently skip files that fail to read
      }
    }
    // Reset the input so the same file can be re-selected
    e.target.value = '';
  }, []);

  const removeAttachment = useCallback((index: number) => {
    setAttachments(prev => prev.filter((_, i) => i !== index));
  }, []);

  const handleDragOver = useCallback((e: React.DragEvent) => {
    e.preventDefault();
    e.stopPropagation();
  }, []);

  const handleDrop = useCallback(async (e: React.DragEvent) => {
    e.preventDefault();
    e.stopPropagation();
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
        setAttachments(prev => [...prev, {
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
      className="border-t border-border bg-surface-1 px-4 py-3"
      onDragOver={handleDragOver}
      onDrop={handleDrop}
    >
      {/* Token usage bar */}
      {tokenUsage && tokenUsage.contextWindow > 0 && (() => {
        const percentage = Math.min(100, (tokenUsage.totalTokens / tokenUsage.contextWindow) * 100);
        return (
          <div className="flex items-center gap-2 px-1 pb-2 text-[10px] text-muted/60">
            <div className="flex-1 bg-surface-3 rounded h-1">
              <div
                className={`h-1 rounded transition-all duration-500 ${
                  percentage > 80 ? 'bg-red-500' : percentage > 50 ? 'bg-yellow-500' : 'bg-accent'
                }`}
                style={{ width: `${Math.max(percentage, 0.5)}%` }}
              />
            </div>
            <span className="shrink-0 tabular-nums">
              {formatTokens(tokenUsage.totalTokens)} / {formatTokens(tokenUsage.contextWindow)} {t('chat.tokensLabel')}
            </span>
          </div>
        );
      })()}
      {/* Attachment preview */}
      {attachments.length > 0 && (
        <div className="flex flex-wrap gap-2 pb-2">
          {attachments.map((att, i) => (
            <div key={i} className="relative group">
              <img
                src={`data:${att.mediaType};base64,${att.base64Data}`}
                alt={att.originalName}
                className="w-16 h-16 object-cover rounded-md border border-border"
              />
              <button
                onClick={() => removeAttachment(i)}
                className="absolute -top-1.5 -right-1.5 bg-danger text-white rounded-full
                  w-4 h-4 flex items-center justify-center text-[10px] leading-none
                  opacity-0 group-hover:opacity-100 transition-opacity cursor-pointer"
                aria-label={t('chat.removeAttachment')}
              >
                <X className="w-3 h-3" />
              </button>
              <span className="absolute bottom-0 left-0 right-0 bg-black/50 text-white text-[9px] px-1 truncate rounded-b-md">
                {att.originalName}
              </span>
            </div>
          ))}
        </div>
      )}

      <div className="flex items-end gap-2">
        {/* Attachment button */}
        <motion.button
          whileTap={{ scale: 0.95 }}
          onClick={() => fileInputRef.current?.click()}
          disabled={disabled || isStreaming}
          className="shrink-0 h-10 w-10 flex items-center justify-center
            rounded-lg text-text-tertiary hover:bg-surface-2 hover:text-text-secondary
            transition-colors duration-fast ease-out cursor-pointer
            disabled:opacity-40 disabled:pointer-events-none"
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
          ref={textareaRef}
          value={value}
          onChange={(e) => setValue(e.target.value)}
          onKeyDown={handleKeyDown}
          placeholder={t('chat.placeholder')}
          disabled={disabled}
          rows={1}
          className="flex-1 resize-none bg-surface-0 border border-border rounded-lg
            text-sm text-text-primary placeholder:text-text-tertiary
            px-3.5 py-2.5 outline-none
            transition-all duration-fast ease-out
            hover:border-border-hover
            focus:border-accent focus:ring-1 focus:ring-accent/30
            disabled:opacity-40 disabled:pointer-events-none"
        />

        {isStreaming ? (
          <motion.button
            whileTap={{ scale: 0.95 }}
            onClick={onStop}
            className="shrink-0 h-10 w-10 flex items-center justify-center
              rounded-lg bg-danger/10 text-danger hover:bg-danger/20
              transition-colors duration-fast ease-out cursor-pointer"
            aria-label={t('chat.stop')}
          >
            <Square className="h-4 w-4" />
          </motion.button>
        ) : (
          <motion.button
            whileTap={{ scale: 0.95 }}
            onClick={handleSend}
            disabled={disabled || (!value.trim() && attachments.length === 0)}
            className="shrink-0 h-10 w-10 flex items-center justify-center
              rounded-lg bg-accent text-white hover:bg-accent-hover
              transition-colors duration-fast ease-out cursor-pointer
              disabled:opacity-40 disabled:pointer-events-none"
            aria-label={t('chat.send')}
          >
            <Send className="h-4 w-4" />
          </motion.button>
        )}
      </div>
    </div>
  );
}
