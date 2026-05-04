import { useState, useRef, useCallback, useEffect } from "react";
import { Send, Square, Paperclip, X, FileText } from "lucide-react";
import { toast } from "sonner";
import { useTranslation } from "../../i18n";
import type { ImageAttachment } from "../../types/conversation";
import { CheckpointMenu } from "./CheckpointMenu";
import { VoiceInputButton } from "./VoiceInputButton";
import { EmojiPicker } from "./EmojiPicker";

const ALLOWED_MIME_TYPES = new Set([
  "image/jpeg",
  "image/png",
  "image/gif",
  "image/webp",
  "application/pdf",
  "text/plain",
  "text/markdown",
  "text/x-markdown",
  "text/csv",
  "application/json",
  "application/vnd.openxmlformats-officedocument.wordprocessingml.document",
  "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet",
  "application/vnd.openxmlformats-officedocument.presentationml.presentation",
  "application/msword",
  "application/vnd.ms-excel",
  "application/vnd.ms-powerpoint",
]);

interface ChatInputProps {
  onSend: (message: string, attachments?: ImageAttachment[]) => void;
  onStop: () => void;
  isStreaming: boolean;
  disabled: boolean;
  conversationId?: string;
  onRestoreCheckpoint?: () => void;
  prefillText?: string;
}

interface ChatDraftState {
  value: string;
  attachments: ImageAttachment[];
}

export function ChatInput({
  onSend,
  onStop,
  isStreaming,
  disabled,
  conversationId,
  onRestoreCheckpoint,
  prefillText,
}: ChatInputProps) {
  const { t } = useTranslation();
  const draftKey = conversationId ?? "__new__";
  const [value, setValue] = useState("");
  const [attachments, setAttachments] = useState<ImageAttachment[]>([]);
  const [isDragging, setIsDragging] = useState(false);
  const textareaRef = useRef<HTMLTextAreaElement>(null);
  const fileInputRef = useRef<HTMLInputElement>(null);
  const dragCounterRef = useRef(0);
  const draftsRef = useRef<Record<string, ChatDraftState>>({});

  useEffect(() => {
    const draft = draftsRef.current[draftKey];
    setValue(draft?.value ?? "");
    setAttachments(draft?.attachments ?? []);
    setTimeout(() => {
      if (textareaRef.current) {
        textareaRef.current.style.height = "auto";
      }
    }, 0);
  }, [draftKey]);

  useEffect(() => {
    draftsRef.current[draftKey] = { value, attachments };
  }, [attachments, draftKey, value]);

  // Accept prefilled text from outside (e.g. suggestion cards)
  useEffect(() => {
    if (prefillText != null && prefillText !== "") {
      setValue(prefillText);
      draftsRef.current[draftKey] = { value: prefillText, attachments };
      setTimeout(() => textareaRef.current?.focus(), 0);
    }
  }, [attachments, draftKey, prefillText]);

  // Auto-resize textarea
  const adjustHeight = useCallback(() => {
    const el = textareaRef.current;
    if (!el) return;
    el.style.height = "auto";
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
    onSend(
      trimmed || t("chat.imageMessage"),
      attachments.length > 0 ? attachments : undefined,
    );
    draftsRef.current[draftKey] = { value: "", attachments: [] };
    setValue("");
    setAttachments([]);
    setTimeout(() => {
      if (textareaRef.current) {
        textareaRef.current.style.height = "auto";
      }
    }, 0);
  }, [attachments, draftKey, onSend, t, value]);

  const handleKeyDown = useCallback(
    (e: React.KeyboardEvent) => {
      if (e.key === "Enter" && !e.shiftKey) {
        e.preventDefault();
        if (!disabled) {
          handleSend();
        }
      }
    },
    [handleSend, disabled],
  );

  const addAttachmentFromDataUrl = useCallback(
    (dataUrl: string, name: string): boolean => {
      const match = dataUrl.match(/^data:([^;]+);base64,(.+)$/);
      if (!match) return false;
      const [, mediaType, base64Data] = match;
      if (!ALLOWED_MIME_TYPES.has(mediaType)) return false;
      setAttachments((prev) => [
        ...prev,
        { base64Data, mediaType, originalName: name },
      ]);
      return true;
    },
    [],
  );

  const addAttachment = useCallback(
    async (blob: Blob, name: string): Promise<boolean> => {
      const reader = new FileReader();
      const result = await new Promise<string>((resolve, reject) => {
        reader.onload = () => resolve(reader.result as string);
        reader.onerror = reject;
        reader.readAsDataURL(blob);
      });
      return addAttachmentFromDataUrl(result, name);
    },
    [addAttachmentFromDataUrl],
  );

  const handleFileSelect = useCallback(
    async (e: React.ChangeEvent<HTMLInputElement>) => {
      if (isStreaming) return;
      const files = e.target.files;
      if (!files) return;
      for (const file of Array.from(files)) {
        try {
          await addAttachment(file, file.name);
        } catch {
          // Silently skip files that fail to read
        }
      }
      e.target.value = "";
    },
    [addAttachment, isStreaming],
  );

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
    if (e.dataTransfer.types.includes("Files")) {
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

  const handleDrop = useCallback(
    async (e: React.DragEvent) => {
      e.preventDefault();
      e.stopPropagation();
      dragCounterRef.current = 0;
      setIsDragging(false);
      if (isStreaming) return;
      const files = e.dataTransfer.files;
      if (!files) return;
      for (const file of Array.from(files)) {
        if (!ALLOWED_MIME_TYPES.has(file.type)) continue;
        try {
          await addAttachment(file, file.name);
        } catch {
          // Silently skip
        }
      }
    },
    [addAttachment, isStreaming],
  );

  const handlePaste = useCallback(
    async (e: React.ClipboardEvent) => {
      if (isStreaming) return;
      const clipboardData = e.clipboardData;
      if (!clipboardData) return;

      // --- Synchronously collect all image files BEFORE any async work ---
      const imageFiles: { file: File; name: string }[] = [];

      // 1. Check clipboardData.files
      if (clipboardData.files.length > 0) {
        for (const file of Array.from(clipboardData.files)) {
          if (file.type.startsWith("image/")) {
            imageFiles.push({
              file,
              name: file.name || `pasted-image-${Date.now()}.png`,
            });
          }
        }
      }

      // 2. Check clipboardData.items (clipboard items API fallback)
      if (imageFiles.length === 0 && clipboardData.items) {
        for (const item of Array.from(clipboardData.items)) {
          if (!item.type.startsWith("image/")) continue;
          const blob = item.getAsFile();
          if (!blob) continue;
          const ext = item.type.split("/")[1] || "png";
          imageFiles.push({
            file: blob,
            name: `pasted-image-${Date.now()}.${ext}`,
          });
        }
      }

      // 3. If we found image files, preventDefault IMMEDIATELY (synchronous)
      if (imageFiles.length > 0) {
        e.preventDefault();
        // Now process asynchronously
        for (const { file, name } of imageFiles) {
          try {
            await addAttachment(file, name);
          } catch (err) {
            console.error("Failed to add image attachment:", err);
            toast.error(t("chat.pasteImageFailed"));
          }
        }
        return;
      }

      // 4. HTML data-URL fallback (no async needed)
      const html = clipboardData.getData("text/html") ?? "";
      const dataUrlMatch = html.match(/src=["'](data:image\/[^"']+)["']/i);
      if (dataUrlMatch) {
        const dataUrl = dataUrlMatch[1];
        const ext = dataUrl.match(/^data:image\/([^;]+)/i)?.[1] || "png";
        const name = `pasted-image-${Date.now()}.${ext}`;
        if (addAttachmentFromDataUrl(dataUrl, name)) {
          e.preventDefault();
        }
      }
    },
    [addAttachment, addAttachmentFromDataUrl, isStreaming, t],
  );

  return (
    <div
      data-testid="chat-input"
      className={`relative border-t border-border bg-surface-1 px-4 py-3 transition-colors ${
        isDragging ? "ring-2 ring-accent/50 bg-accent-subtle" : ""
      }`}
      onDragOver={handleDragOver}
      onDragEnter={handleDragEnter}
      onDragLeave={handleDragLeave}
      onDrop={handleDrop}
    >
      {isDragging && (
        <div className="absolute inset-0 z-10 flex items-center justify-center rounded-lg border-2 border-dashed border-accent bg-accent-subtle/50 pointer-events-none">
          <span className="text-sm font-medium text-accent">
            {t("chat.dragDropHint")}
          </span>
        </div>
      )}

      {attachments.length > 0 && (
        <div className="flex flex-wrap gap-2 pb-2">
          {attachments.map((att, i) => (
            <div key={i} className="relative group">
              {att.mediaType.startsWith("image/") ? (
                <img
                  src={`data:${att.mediaType};base64,${att.base64Data}`}
                  alt={att.originalName}
                  className="h-16 w-16 rounded-md border border-border object-cover"
                />
              ) : (
                <div className="h-16 w-16 rounded-md border border-border bg-surface-2 flex items-center justify-center">
                  <FileText className="h-6 w-6 text-text-tertiary" />
                </div>
              )}
              <button
                onClick={() => removeAttachment(i)}
                className="absolute -right-1.5 -top-1.5 flex h-4 w-4 items-center justify-center rounded-full bg-danger text-[10px] leading-none text-white opacity-0 transition-opacity cursor-pointer group-hover:opacity-100"
                aria-label={t("chat.removeAttachment")}
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
        <button
          onClick={() => fileInputRef.current?.click()}
          disabled={disabled || isStreaming}
          className="flex h-10 w-10 shrink-0 items-center justify-center rounded-lg text-text-tertiary transition-colors duration-fast ease-out cursor-pointer hover:bg-surface-2 hover:text-text-secondary disabled:pointer-events-none disabled:opacity-40"
          aria-label={t("chat.attachImage")}
        >
          <Paperclip className="h-4 w-4" />
        </button>
        <input
          ref={fileInputRef}
          type="file"
          accept="image/jpeg,image/png,image/gif,image/webp,.pdf,.txt,.md,.csv,.json,.docx,.xlsx,.pptx,.doc,.xls,.ppt"
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
          onPaste={handlePaste}
          placeholder={t("chat.placeholder")}
          disabled={disabled}
          rows={1}
          className="flex-1 resize-none rounded-lg border border-border bg-surface-0 px-3.5 py-2.5 text-sm text-text-primary placeholder:text-text-tertiary outline-none transition-all duration-fast ease-out hover:border-border-hover focus:border-accent focus:ring-1 focus:ring-accent/30 disabled:pointer-events-none disabled:opacity-40"
        />

        <VoiceInputButton
          onTranscript={(text) =>
            setValue((prev) => prev + (prev ? " " : "") + text)
          }
          disabled={disabled || isStreaming}
        />

        <EmojiPicker
          onEmojiSelect={(emoji) => {
            setValue((prev) => prev + emoji);
            textareaRef.current?.focus();
          }}
          disabled={disabled || isStreaming}
        />

        <button
          onClick={handleSend}
          disabled={disabled || (!value.trim() && attachments.length === 0)}
          data-testid="chat-send"
          className="flex h-10 w-10 shrink-0 items-center justify-center rounded-lg bg-accent text-white transition-colors duration-fast ease-out cursor-pointer hover:bg-accent-hover disabled:pointer-events-none disabled:opacity-40"
          aria-label={t("chat.send")}
        >
          <Send className="h-4 w-4" />
        </button>

        {isStreaming && (
          <button
            onClick={onStop}
            className="flex h-10 w-10 shrink-0 items-center justify-center rounded-lg bg-danger/10 text-danger transition-colors duration-fast ease-out cursor-pointer hover:bg-danger/20"
            aria-label={t("chat.stop")}
          >
            <Square className="h-4 w-4" />
          </button>
        )}
      </div>

      {conversationId && onRestoreCheckpoint && (
        <div className="mt-2 flex justify-end">
          <CheckpointMenu
            conversationId={conversationId}
            onRestore={onRestoreCheckpoint}
          />
        </div>
      )}
    </div>
  );
}
