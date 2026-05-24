import { useState, useRef, useCallback, useEffect } from "react";
import { ArrowUp, Square, Paperclip, X, FileText, Workflow, ChevronDown, ArchiveRestore, Loader2 } from "lucide-react";
import { toast } from "sonner";
import { useTranslation } from "../../i18n";
import type { Conversation, ImageAttachment } from "../../types/conversation";
import type { WorkflowCatalogTemplate } from "../../lib/api";
import * as api from "../../lib/api";
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

const ALLOWED_EXTENSIONS = new Set([
  ".jpg",
  ".jpeg",
  ".png",
  ".gif",
  ".webp",
  ".pdf",
  ".txt",
  ".md",
  ".csv",
  ".json",
  ".docx",
  ".xlsx",
  ".pptx",
  ".doc",
  ".xls",
  ".ppt",
]);

const MIME_BY_EXTENSION = new Map<string, string>([
  [".jpg", "image/jpeg"],
  [".jpeg", "image/jpeg"],
  [".png", "image/png"],
  [".gif", "image/gif"],
  [".webp", "image/webp"],
  [".pdf", "application/pdf"],
  [".txt", "text/plain"],
  [".md", "text/markdown"],
  [".csv", "text/csv"],
  [".json", "application/json"],
  [".docx", "application/vnd.openxmlformats-officedocument.wordprocessingml.document"],
  [".xlsx", "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet"],
  [".pptx", "application/vnd.openxmlformats-officedocument.presentationml.presentation"],
  [".doc", "application/msword"],
  [".xls", "application/vnd.ms-excel"],
  [".ppt", "application/vnd.ms-powerpoint"],
]);

function getFileExtension(name: string): string {
  const dot = name.lastIndexOf(".");
  return dot >= 0 ? name.slice(dot).toLowerCase() : "";
}

function getAllowedAttachmentMediaType(mediaType: string | undefined, name: string): string | null {
  const normalized = (mediaType ?? "").trim().toLowerCase();
  if (normalized && ALLOWED_MIME_TYPES.has(normalized)) {
    return normalized;
  }

  const ext = getFileExtension(name);
  if (!ALLOWED_EXTENSIONS.has(ext)) {
    return null;
  }

  return MIME_BY_EXTENSION.get(ext) ?? "application/octet-stream";
}

interface ChatInputProps {
  onSend: (message: string, attachments?: ImageAttachment[]) => void;
  onStop: () => void;
  isStreaming: boolean;
  disabled: boolean;
  conversationId?: string;
  onRestoreCheckpoint?: () => void;
  onBranchCheckpoint?: (conversation: Conversation) => void;
  prefillText?: string;
  onCompact?: () => void;
  isCompacting?: boolean;
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
  onBranchCheckpoint,
  prefillText,
  onCompact,
  isCompacting = false,
}: ChatInputProps) {
  const { t } = useTranslation();
  const draftKey = conversationId ?? "__new__";
  const [value, setValue] = useState("");
  const [attachments, setAttachments] = useState<ImageAttachment[]>([]);
  const [isDragging, setIsDragging] = useState(false);
  const [workflowTemplates, setWorkflowTemplates] = useState<WorkflowCatalogTemplate[]>([]);
  const [workflowCatalogOpen, setWorkflowCatalogOpen] = useState(false);
  const [workflowCatalogLoading, setWorkflowCatalogLoading] = useState(false);
  const textareaRef = useRef<HTMLTextAreaElement>(null);
  const fileInputRef = useRef<HTMLInputElement>(null);
  const dragCounterRef = useRef(0);
  const draftsRef = useRef<Record<string, ChatDraftState>>({});
  const inputLocked = disabled || isCompacting;
  const attachmentLocked = inputLocked || isStreaming;

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
    const minHeight = 96;
    const maxHeight = lineHeight * 9 + 20;
    el.style.height = `${Math.max(minHeight, Math.min(el.scrollHeight, maxHeight))}px`;
  }, []);

  useEffect(() => {
    adjustHeight();
  }, [value, adjustHeight]);

  useEffect(() => {
    let cancelled = false;
    setWorkflowCatalogLoading(true);
    api.listWorkflowTemplates()
      .then((templates) => {
        if (!cancelled && Array.isArray(templates)) {
          setWorkflowTemplates(templates);
        }
      })
      .catch((err) => {
        console.warn("Failed to load workflow templates:", err);
      })
      .finally(() => {
        if (!cancelled) {
          setWorkflowCatalogLoading(false);
        }
      });

    return () => {
      cancelled = true;
    };
  }, []);

  const handleSend = useCallback(() => {
    if (inputLocked) return;
    const trimmed = value.trim();
    if (!trimmed && attachments.length === 0) return;
    if (isStreaming && (!trimmed || attachments.length > 0)) {
      toast.error("Attachments cannot be added while the agent is already running.");
      return;
    }
    if (trimmed === "/compact" && attachments.length === 0 && onCompact) {
      onCompact();
      draftsRef.current[draftKey] = { value: "", attachments: [] };
      setValue("");
      setAttachments([]);
      return;
    }
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
  }, [attachments, draftKey, inputLocked, isStreaming, onCompact, onSend, t, value]);

  const handleKeyDown = useCallback(
    (e: React.KeyboardEvent) => {
      if (e.key === "Enter" && !e.shiftKey) {
        e.preventDefault();
        if (!inputLocked) {
          handleSend();
        }
      }
    },
    [handleSend, inputLocked],
  );

  const addAttachmentFromDataUrl = useCallback(
    (dataUrl: string, name: string): boolean => {
      const match = dataUrl.match(/^data:([^;]+);base64,(.+)$/);
      if (!match) return false;
      const [, mediaType, base64Data] = match;
      const allowedMediaType = getAllowedAttachmentMediaType(mediaType, name);
      if (!allowedMediaType) return false;
      setAttachments((prev) => [
        ...prev,
        { base64Data, mediaType: allowedMediaType, originalName: name },
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
    if (attachmentLocked) return;
    dragCounterRef.current += 1;
    if (e.dataTransfer.types.includes("Files")) {
      setIsDragging(true);
    }
  }, [attachmentLocked]);

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
      if (attachmentLocked) return;
      const files = e.dataTransfer.files;
      if (!files) return;
      for (const file of Array.from(files)) {
        if (!getAllowedAttachmentMediaType(file.type, file.name)) continue;
        try {
          await addAttachment(file, file.name);
        } catch {
          // Silently skip
        }
      }
    },
    [addAttachment, attachmentLocked],
  );

  const handlePaste = useCallback(
    async (e: React.ClipboardEvent) => {
      if (attachmentLocked) return;
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
    [addAttachment, addAttachmentFromDataUrl, attachmentLocked, t],
  );

  const applyWorkflowTemplate = useCallback((template: WorkflowCatalogTemplate) => {
    const prompt = template.promptTemplate.trimEnd();
    setValue((currentValue) => {
      const current = currentValue.trim();
      const nextValue = current ? `${prompt}\n\n${current}` : prompt;
      draftsRef.current[draftKey] = { value: nextValue, attachments };
      return nextValue;
    });
    setWorkflowCatalogOpen(false);
    requestAnimationFrame(() => {
      textareaRef.current?.focus();
      adjustHeight();
    });
  }, [adjustHeight, attachments, draftKey]);

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

      {workflowCatalogOpen && (
        <div
          data-testid="workflow-catalog-panel"
          className="absolute bottom-full left-4 right-4 z-30 mb-2 overflow-hidden rounded-lg border border-border/70 bg-surface-0 shadow-2xl shadow-black/30"
        >
          <div className="flex items-center gap-2 border-b border-border/60 px-3 py-2">
            <Workflow className="h-4 w-4 text-accent" />
            <span className="text-sm font-medium text-text-primary">{t("chat.workflows")}</span>
            <span className="text-xs tabular-nums text-text-tertiary">
              {workflowCatalogLoading
                ? t("common.loading")
                : t("chat.workflowTemplateCount", { count: String(workflowTemplates.length) })}
            </span>
            <button
              type="button"
              onClick={() => setWorkflowCatalogOpen(false)}
              className="ml-auto flex h-7 w-7 items-center justify-center rounded-md text-text-tertiary transition-colors hover:bg-surface-2 hover:text-text-primary"
              aria-label={t("common.close")}
            >
              <X className="h-4 w-4" />
            </button>
          </div>

          <div className="grid max-h-72 gap-2 overflow-y-auto p-2 sm:grid-cols-2 lg:grid-cols-3">
            {workflowTemplates.map((template) => (
              <button
                key={template.id}
                type="button"
                onClick={() => applyWorkflowTemplate(template)}
                className="min-h-[112px] rounded-lg border border-border/70 bg-surface-1/70 p-3 text-left transition-colors hover:border-accent/60 hover:bg-accent-subtle/40 focus:outline-none focus:ring-2 focus:ring-accent/30"
                aria-label={`${template.label} workflow`}
              >
                <div className="flex items-start justify-between gap-2">
                  <span className="text-sm font-medium leading-5 text-text-primary">
                    {template.label}
                  </span>
                  <span className="shrink-0 rounded-md border border-border/60 bg-surface-0 px-1.5 py-0.5 text-[10px] tabular-nums text-text-tertiary">
                    {t("chat.workflowTasks", { count: String(template.tasks.length) })}
                  </span>
                </div>
                <div className="mt-1.5 line-clamp-2 text-xs leading-5 text-text-secondary">
                  {template.description}
                </div>
                <div className="mt-2 flex flex-wrap gap-1">
                  {template.tasks.slice(0, 3).map((task) => (
                    <span
                      key={task.id}
                      className="rounded-md bg-surface-0 px-1.5 py-0.5 text-[10px] text-text-tertiary"
                    >
                      {task.roleLabel}
                    </span>
                  ))}
                </div>
              </button>
            ))}
            {!workflowCatalogLoading && workflowTemplates.length === 0 && (
              <div className="col-span-full px-3 py-6 text-center text-sm text-text-tertiary">
                {t("chat.workflowUnavailable")}
              </div>
            )}
          </div>
        </div>
      )}

      <div className="overflow-hidden rounded-xl border border-border/80 bg-surface-0 shadow-[0_12px_32px_rgba(0,0,0,0.16)] ring-1 ring-white/[0.03] transition-colors duration-fast focus-within:border-accent/55 focus-within:ring-accent/20">
        {attachments.length > 0 && (
          <div className="flex flex-wrap gap-2 border-b border-border/35 px-3 py-2.5">
            {attachments.map((att, i) => (
              <div key={i} className="relative group">
                {att.mediaType.startsWith("image/") ? (
                  <img
                    src={`data:${att.mediaType};base64,${att.base64Data}`}
                    alt={att.originalName}
                    className="h-14 w-14 rounded-md border border-border object-cover"
                  />
                ) : (
                  <div className="h-14 w-14 rounded-md border border-border bg-surface-2 flex items-center justify-center">
                    <FileText className="h-5 w-5 text-text-tertiary" />
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

        <input
          ref={fileInputRef}
          type="file"
          accept="image/jpeg,image/png,image/gif,image/webp,.jpg,.jpeg,.png,.gif,.webp,.pdf,.txt,.md,.csv,.json,.docx,.xlsx,.pptx,.doc,.xls,.ppt"
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
          placeholder={isCompacting ? `${t("chat.compacting")} (>_<)` : t("chat.placeholder")}
          disabled={inputLocked}
          rows={1}
          className="block min-h-24 w-full resize-none overflow-y-auto bg-transparent px-4 pb-3 pt-3.5 text-sm leading-6 text-text-primary placeholder:text-text-tertiary outline-none disabled:pointer-events-none disabled:opacity-40"
        />

        <div className="flex min-h-11 items-center justify-between gap-3 border-t border-border/35 px-2.5 py-2">
          <div className="flex min-w-0 items-center gap-1.5">
            <button
              type="button"
              data-testid="workflow-catalog-trigger"
              onClick={() => setWorkflowCatalogOpen((open) => !open)}
              disabled={attachmentLocked}
              className="flex h-8 shrink-0 items-center gap-1.5 rounded-md px-2 text-xs font-medium text-text-secondary transition-colors duration-fast ease-out hover:bg-surface-2 hover:text-text-primary disabled:pointer-events-none disabled:opacity-40"
              aria-label={t("chat.workflows")}
              aria-expanded={workflowCatalogOpen}
            >
              <Workflow className="h-3.5 w-3.5" />
              <span className="hidden sm:inline">{t("chat.workflows")}</span>
              <ChevronDown className={`h-3 w-3 transition-transform ${workflowCatalogOpen ? "rotate-180" : ""}`} />
            </button>

            <button
              onClick={() => fileInputRef.current?.click()}
              disabled={attachmentLocked}
              className="flex h-8 w-8 shrink-0 items-center justify-center rounded-md text-text-tertiary transition-colors duration-fast ease-out cursor-pointer hover:bg-surface-2 hover:text-text-secondary disabled:pointer-events-none disabled:opacity-40"
              aria-label={t("chat.attachImage")}
            >
              <Paperclip className="h-3.5 w-3.5" />
            </button>
            {conversationId && onCompact && (
              <button
                type="button"
                data-testid="chat-compact"
                onClick={onCompact}
                disabled={attachmentLocked}
                className="flex h-8 w-8 shrink-0 items-center justify-center rounded-md text-text-tertiary transition-colors duration-fast ease-out cursor-pointer hover:bg-surface-2 hover:text-text-secondary disabled:pointer-events-none disabled:opacity-40"
                aria-label={t("chat.compact")}
                title="/compact"
              >
                {isCompacting ? (
                  <Loader2 className="h-3.5 w-3.5 animate-spin" />
                ) : (
                  <ArchiveRestore className="h-3.5 w-3.5" />
                )}
              </button>
            )}
            {conversationId && onRestoreCheckpoint && (
              <CheckpointMenu
                conversationId={conversationId}
                onRestore={onRestoreCheckpoint}
                onBranch={onBranchCheckpoint}
              />
            )}
          </div>

          <div className="flex shrink-0 items-center gap-1.5">
            <VoiceInputButton
              onTranscript={(text) =>
                setValue((prev) => prev + (prev ? " " : "") + text)
              }
              disabled={attachmentLocked}
            />

            <EmojiPicker
              onEmojiSelect={(emoji) => {
                setValue((prev) => prev + emoji);
                textareaRef.current?.focus();
              }}
              disabled={attachmentLocked}
            />

            {isStreaming && (
              <button
                onClick={onStop}
                data-testid="chat-stop"
                className="flex h-8 w-8 shrink-0 items-center justify-center rounded-md bg-danger/10 text-danger transition-colors duration-fast ease-out cursor-pointer hover:bg-danger/20"
                aria-label={t("chat.stop")}
              >
                <Square className="h-3.5 w-3.5" />
              </button>
            )}
            <button
              onClick={handleSend}
              disabled={
                inputLocked ||
                (isStreaming
                  ? !value.trim() || attachments.length > 0
                  : !value.trim() && attachments.length === 0)
              }
              data-testid="chat-send"
              className="flex h-8 w-8 shrink-0 items-center justify-center rounded-full border border-text-primary/10 bg-text-primary text-surface-0 shadow-[0_8px_20px_rgba(0,0,0,0.22)] transition-[background-color,border-color,color,box-shadow,transform] duration-fast ease-out cursor-pointer hover:-translate-y-0.5 hover:bg-text-secondary hover:shadow-[0_10px_24px_rgba(0,0,0,0.28)] focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-accent/35 disabled:pointer-events-none disabled:translate-y-0 disabled:border-border disabled:bg-surface-2 disabled:text-text-tertiary disabled:shadow-none"
              aria-label={isStreaming ? t("chat.steeringMessage") : t("chat.send")}
            >
              <ArrowUp className="h-4 w-4" strokeWidth={2.4} />
            </button>
          </div>
        </div>
      </div>
    </div>
  );
}
