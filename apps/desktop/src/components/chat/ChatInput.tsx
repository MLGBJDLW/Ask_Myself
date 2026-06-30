import { useState, useRef, useCallback, useEffect, useMemo, type ReactNode } from "react";
import { ArrowUp, Square, Paperclip, X, FileText, Workflow, ChevronDown, ArchiveRestore, Loader2, Command, BrainCircuit } from "lucide-react";
import { toast } from "sonner";
import { useTranslation, type TranslationKey } from "../../i18n";
import type { ArtifactPayload, Conversation, ImageAttachment } from "../../types/conversation";
import type { Skill } from "../../types/extensions";
import type { AgentExecutionMode, WorkflowCatalogTemplate } from "../../lib/api";
import * as api from "../../lib/api";
import {
  buildSlashCommandOptions,
  getMatchingSlashCommands,
  getSlashCommandTrigger,
  insertSlashCommand,
  resolveSlashCommandMessage,
  type SlashCommandKind,
  type SlashCommandOption,
} from "../../lib/slashCommands";
import {
  buildGoalContinuationLlmContext,
  mergeGoalContextArtifact,
  type ActiveGoalContext,
} from "../../lib/goalContext";
import { buildWorkflowBatchPrompt } from "../../lib/workflowPrompts";
import {
  collectPastedImageFiles,
  getAllowedAttachmentMediaType,
  getPastedImageDataUrl,
} from "../../lib/chatAttachments";
import { CheckpointMenu } from "./CheckpointMenu";
import { VoiceInputButton } from "./VoiceInputButton";
import { EmojiPicker } from "./EmojiPicker";

const LLM_CONTEXT_CONTENT_ARTIFACT_KEY = "llmContextContent";

export interface ChatInputSendOptions {
  skillIds?: string[];
  userArtifacts?: ArtifactPayload | null;
  executionMode?: AgentExecutionMode;
  taskOrchestratorRunId?: string | null;
}

interface ChatInputProps {
  onSend: (message: string, attachments?: ImageAttachment[], options?: ChatInputSendOptions) => void;
  onStop: () => void;
  isStreaming: boolean;
  disabled: boolean;
  conversationId?: string;
  inputHistory?: string[];
  sessionControls?: ReactNode;
  onRestoreCheckpoint?: () => void;
  onBranchCheckpoint?: (conversation: Conversation) => void;
  prefillText?: string;
  onCompact?: () => void;
  isCompacting?: boolean;
  planModeEnabled?: boolean;
  onPlanModeChange?: (enabled: boolean) => void;
  activeGoalContext?: ActiveGoalContext | null;
}

interface ChatDraftState {
  value: string;
  attachments: ImageAttachment[];
}

interface StoredChatDraftState {
  value: string;
  updatedAt: number;
}

type SlashCommandTab = "all" | SlashCommandKind;

const NEW_CONVERSATION_DRAFT_KEY = "__new__";
const CHAT_INPUT_DRAFT_STORAGE_KEY = "chat-input-drafts-v1";
const MAX_STORED_CHAT_INPUT_DRAFTS = 100;
const MAX_INPUT_HISTORY_ITEMS = 100;
const chatInputDrafts: Record<string, ChatDraftState> = {};

const SLASH_COMMAND_TABS: SlashCommandTab[] = ["all", "command", "skill", "workflow"];
const LOCALIZED_COMMON_SLASH_COMMANDS = new Set([
  "plan",
  "goal",
  "review",
  "debug",
  "refactor",
  "test",
  "docs",
  "research",
  "summarize",
  "compare",
  "tasks",
  "commit",
  "image",
  "skills",
  "workflow",
  "compact",
]);

function commonSlashCommandKey(name: string, field: "title" | "description"): TranslationKey | null {
  return LOCALIZED_COMMON_SLASH_COMMANDS.has(name)
    ? (`chat.slashCommand.${name}.${field}` as TranslationKey)
    : null;
}

function cloneDraftState(draft: ChatDraftState): ChatDraftState {
  return {
    value: draft.value,
    attachments: draft.attachments.slice(),
  };
}

function readStoredChatInputDrafts(): Record<string, StoredChatDraftState> {
  try {
    const raw = sessionStorage.getItem(CHAT_INPUT_DRAFT_STORAGE_KEY);
    if (!raw) return {};
    const parsed = JSON.parse(raw) as unknown;
    if (!parsed || typeof parsed !== "object") return {};

    const drafts: Record<string, StoredChatDraftState> = {};
    for (const [key, value] of Object.entries(parsed as Record<string, unknown>)) {
      if (!key || !value || typeof value !== "object") continue;
      const row = value as Record<string, unknown>;
      if (typeof row.value !== "string") continue;
      const updatedAt = typeof row.updatedAt === "number" && Number.isFinite(row.updatedAt)
        ? row.updatedAt
        : 0;
      drafts[key] = { value: row.value, updatedAt };
    }
    return drafts;
  } catch {
    return {};
  }
}

function writeStoredChatInputDrafts(drafts: Record<string, StoredChatDraftState>) {
  try {
    const entries = Object.entries(drafts)
      .sort(([, a], [, b]) => b.updatedAt - a.updatedAt)
      .slice(0, MAX_STORED_CHAT_INPUT_DRAFTS);
    if (entries.length === 0) {
      sessionStorage.removeItem(CHAT_INPUT_DRAFT_STORAGE_KEY);
      return;
    }
    sessionStorage.setItem(CHAT_INPUT_DRAFT_STORAGE_KEY, JSON.stringify(Object.fromEntries(entries)));
  } catch {
    // ignore storage failures
  }
}

function readChatInputDraft(draftKey: string): ChatDraftState {
  const cached = chatInputDrafts[draftKey];
  if (cached) return cloneDraftState(cached);

  const stored = readStoredChatInputDrafts()[draftKey];
  const draft = { value: stored?.value ?? "", attachments: [] };
  chatInputDrafts[draftKey] = cloneDraftState(draft);
  return draft;
}

function persistChatInputDraft(draftKey: string, draft: ChatDraftState) {
  chatInputDrafts[draftKey] = cloneDraftState(draft);

  const storedDrafts = readStoredChatInputDrafts();
  if (draft.value.length > 0) {
    storedDrafts[draftKey] = { value: draft.value, updatedAt: Date.now() };
  } else {
    delete storedDrafts[draftKey];
  }
  writeStoredChatInputDrafts(storedDrafts);
}

function clearChatInputDraft(draftKey: string) {
  delete chatInputDrafts[draftKey];

  const storedDrafts = readStoredChatInputDrafts();
  if (!(draftKey in storedDrafts)) return;
  delete storedDrafts[draftKey];
  writeStoredChatInputDrafts(storedDrafts);
}

function normalizeInputHistory(inputHistory: readonly string[]): string[] {
  const normalized: string[] = [];
  for (const item of inputHistory) {
    const value = item.trim();
    if (!value) continue;
    if (normalized[normalized.length - 1] === value) continue;
    normalized.push(value);
  }
  return normalized.slice(-MAX_INPUT_HISTORY_ITEMS);
}

function isCaretAtInputHistoryBoundary(
  el: HTMLTextAreaElement,
  direction: "up" | "down",
): boolean {
  if (el.selectionStart !== el.selectionEnd) return false;
  const value = el.value;
  if (!value) return direction === "up";
  if (direction === "up") {
    return !value.slice(0, el.selectionStart).includes("\n");
  }
  return !value.slice(el.selectionEnd).includes("\n");
}

export function ChatInput({
  onSend,
  onStop,
  isStreaming,
  disabled,
  conversationId,
  inputHistory = [],
  sessionControls,
  onRestoreCheckpoint,
  onBranchCheckpoint,
  prefillText,
  onCompact,
  isCompacting = false,
  planModeEnabled,
  onPlanModeChange,
  activeGoalContext,
}: ChatInputProps) {
  const { t } = useTranslation();
  const draftKey = conversationId ?? NEW_CONVERSATION_DRAFT_KEY;
  const initialDraftRef = useRef<ChatDraftState | null>(null);
  if (initialDraftRef.current === null) {
    initialDraftRef.current = readChatInputDraft(draftKey);
  }
  const [value, setValue] = useState(() => initialDraftRef.current?.value ?? "");
  const [attachments, setAttachments] = useState<ImageAttachment[]>(() => (
    initialDraftRef.current?.attachments ?? []
  ));
  const [loadedDraftKey, setLoadedDraftKey] = useState(draftKey);
  const [isDragging, setIsDragging] = useState(false);
  const [workflowTemplates, setWorkflowTemplates] = useState<WorkflowCatalogTemplate[]>([]);
  const [activeSkills, setActiveSkills] = useState<Skill[]>([]);
  const [workflowCatalogOpen, setWorkflowCatalogOpen] = useState(false);
  const [workflowCatalogLoading, setWorkflowCatalogLoading] = useState(false);
  const [caretPosition, setCaretPosition] = useState(0);
  const [slashSelectedIndex, setSlashSelectedIndex] = useState(0);
  const [slashActiveTab, setSlashActiveTab] = useState<SlashCommandTab>("all");
  const [dismissedSlashToken, setDismissedSlashToken] = useState<string | null>(null);
  const [localPlanModeEnabled, setLocalPlanModeEnabled] = useState(false);
  const textareaRef = useRef<HTMLTextAreaElement>(null);
  const fileInputRef = useRef<HTMLInputElement>(null);
  const slashOptionRefs = useRef<Array<HTMLButtonElement | null>>([]);
  const dragCounterRef = useRef(0);
  const draftsRef = useRef<Record<string, ChatDraftState>>(
    initialDraftRef.current ? { [draftKey]: cloneDraftState(initialDraftRef.current) } : {},
  );
  const historyDraftRef = useRef<{ value: string; cursor: number } | null>(null);
  const inputLocked = disabled || isCompacting;
  const attachmentLocked = inputLocked || isStreaming;
  const effectivePlanModeEnabled = planModeEnabled ?? localPlanModeEnabled;
  const inputHistoryEntries = useMemo(
    () => normalizeInputHistory(inputHistory),
    [inputHistory],
  );
  const [inputHistoryIndex, setInputHistoryIndex] = useState(-1);

  const resetInputHistoryNavigation = useCallback(() => {
    setInputHistoryIndex(-1);
    historyDraftRef.current = null;
  }, []);

  const setPlanMode = useCallback((enabled: boolean) => {
    if (planModeEnabled === undefined) {
      setLocalPlanModeEnabled(enabled);
    }
    onPlanModeChange?.(enabled);
  }, [onPlanModeChange, planModeEnabled]);

  const persistDraft = useCallback((nextValue: string, nextAttachments: ImageAttachment[] = attachments) => {
    const draft = { value: nextValue, attachments: nextAttachments };
    draftsRef.current[draftKey] = cloneDraftState(draft);
    persistChatInputDraft(draftKey, draft);
  }, [attachments, draftKey]);

  useEffect(() => {
    const draft = draftsRef.current[draftKey] ?? readChatInputDraft(draftKey);
    draftsRef.current[draftKey] = cloneDraftState(draft);
    resetInputHistoryNavigation();
    setLoadedDraftKey(draftKey);
    setValue(draft.value);
    setAttachments(draft.attachments);
    setTimeout(() => {
      if (textareaRef.current) {
        textareaRef.current.style.height = "auto";
      }
    }, 0);
  }, [draftKey, resetInputHistoryNavigation]);

  useEffect(() => {
    if (loadedDraftKey !== draftKey) return;
    persistDraft(value, attachments);
  }, [attachments, draftKey, loadedDraftKey, persistDraft, value]);

  // Accept prefilled text from outside (e.g. suggestion cards)
  useEffect(() => {
    if (prefillText != null && prefillText !== "") {
      resetInputHistoryNavigation();
      setValue(prefillText);
      persistDraft(prefillText);
      setTimeout(() => textareaRef.current?.focus(), 0);
    }
  }, [persistDraft, prefillText, resetInputHistoryNavigation]);

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

  const applyInputHistoryValue = useCallback((nextValue: string, cursor: "start" | "end" | number) => {
    setValue(nextValue);
    persistDraft(nextValue);
    const nextCursor = typeof cursor === "number"
      ? Math.max(0, Math.min(cursor, nextValue.length))
      : cursor === "start"
        ? 0
        : nextValue.length;
    setCaretPosition(nextCursor);
    requestAnimationFrame(() => {
      const el = textareaRef.current;
      if (!el) return;
      el.focus();
      el.setSelectionRange(nextCursor, nextCursor);
      adjustHeight();
    });
  }, [adjustHeight, persistDraft]);

  const navigateInputHistory = useCallback((direction: "up" | "down") => {
    if (inputLocked || inputHistoryEntries.length === 0) return false;
    const el = textareaRef.current;
    if (!el || !isCaretAtInputHistoryBoundary(el, direction)) return false;

    if (direction === "up") {
      if (inputHistoryIndex >= inputHistoryEntries.length - 1) return false;
      const nextIndex = inputHistoryIndex + 1;
      if (inputHistoryIndex === -1) {
        historyDraftRef.current = {
          value,
          cursor: el.selectionStart,
        };
      }
      setInputHistoryIndex(nextIndex);
      applyInputHistoryValue(inputHistoryEntries[inputHistoryEntries.length - 1 - nextIndex], "start");
      return true;
    }

    if (inputHistoryIndex === -1) return false;
    const nextIndex = inputHistoryIndex - 1;
    setInputHistoryIndex(nextIndex);
    if (nextIndex === -1) {
      const draft = historyDraftRef.current;
      applyInputHistoryValue(draft?.value ?? "", draft?.cursor ?? "end");
      historyDraftRef.current = null;
      return true;
    }
    applyInputHistoryValue(inputHistoryEntries[inputHistoryEntries.length - 1 - nextIndex], "end");
    return true;
  }, [
    applyInputHistoryValue,
    inputHistoryEntries,
    inputHistoryIndex,
    inputLocked,
    value,
  ]);

  useEffect(() => {
    setCaretPosition((current) => Math.min(current, value.length));
  }, [value.length]);

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

  useEffect(() => {
    let cancelled = false;
    api.listActiveSkills()
      .then((skills) => {
        if (!cancelled && Array.isArray(skills)) {
          setActiveSkills(skills);
        }
      })
      .catch((err) => {
        console.warn("Failed to load skills for slash commands:", err);
      });

    return () => {
      cancelled = true;
    };
  }, []);

  const slashOptions = useMemo(
    () => buildSlashCommandOptions(activeSkills, workflowTemplates),
    [activeSkills, workflowTemplates],
  );
  const slashTrigger = useMemo(
    () => getSlashCommandTrigger(value, caretPosition),
    [caretPosition, value],
  );
  const slashMatches = useMemo(
    () => slashTrigger ? getMatchingSlashCommands(slashOptions, slashTrigger.query, 64) : [],
    [slashOptions, slashTrigger],
  );
  const slashTabCounts = useMemo<Record<SlashCommandTab, number>>(() => ({
    all: slashMatches.length,
    command: slashMatches.filter((option) => option.kind === "command").length,
    skill: slashMatches.filter((option) => option.kind === "skill").length,
    workflow: slashMatches.filter((option) => option.kind === "workflow").length,
  }), [slashMatches]);
  const visibleSlashMatches = useMemo(
    () => slashActiveTab === "all"
      ? slashMatches
      : slashMatches.filter((option) => option.kind === slashActiveTab),
    [slashActiveTab, slashMatches],
  );
  const slashEnabledTabs = useMemo(
    () => SLASH_COMMAND_TABS.filter((tab) => tab === "all" || slashTabCounts[tab] > 0),
    [slashTabCounts],
  );
  const slashMenuOpen = Boolean(
    slashTrigger &&
    !inputLocked &&
    dismissedSlashToken !== slashTrigger.token,
  );
  const activeSlashIndex = Math.min(slashSelectedIndex, Math.max(0, visibleSlashMatches.length - 1));
  const activeSlashOption = visibleSlashMatches[activeSlashIndex];

  useEffect(() => {
    setSlashSelectedIndex(0);
    setSlashActiveTab("all");
  }, [slashTrigger?.query, slashMatches.length]);

  useEffect(() => {
    setSlashSelectedIndex(0);
  }, [slashActiveTab]);

  useEffect(() => {
    if (slashMenuOpen && slashActiveTab !== "all" && slashTabCounts[slashActiveTab] === 0) {
      setSlashActiveTab("all");
    }
  }, [slashActiveTab, slashMenuOpen, slashTabCounts]);

  useEffect(() => {
    slashOptionRefs.current.length = visibleSlashMatches.length;
  }, [visibleSlashMatches.length]);

  useEffect(() => {
    if (!slashMenuOpen) return;
    slashOptionRefs.current[activeSlashIndex]?.scrollIntoView({ block: "nearest" });
  }, [activeSlashIndex, slashMenuOpen, visibleSlashMatches]);

  const updateCaretFromTextarea = useCallback(() => {
    const el = textareaRef.current;
    if (el) {
      setCaretPosition(el.selectionStart);
    }
  }, []);

  const applySlashOption = useCallback((option: SlashCommandOption) => {
    if (!slashTrigger) return;
    setDismissedSlashToken(null);

    if (option.action === "openWorkflows") {
      const nextValue = `${value.slice(0, slashTrigger.start)}${value.slice(slashTrigger.end)}`.trimStart();
      setValue(nextValue);
      persistDraft(nextValue);
      setWorkflowCatalogOpen(true);
      requestAnimationFrame(() => {
        textareaRef.current?.focus();
        setCaretPosition(textareaRef.current?.selectionStart ?? nextValue.length);
        adjustHeight();
      });
      return;
    }

    if (option.action === "planMode") {
      const nextValue = `${value.slice(0, slashTrigger.start)}${value.slice(slashTrigger.end)}`.trimStart();
      setValue(nextValue);
      persistDraft(nextValue);
      setPlanMode(true);
      requestAnimationFrame(() => {
        textareaRef.current?.focus();
        setCaretPosition(textareaRef.current?.selectionStart ?? nextValue.length);
        adjustHeight();
      });
      return;
    }

    const next = insertSlashCommand(value, slashTrigger, option);
    setValue(next.value);
    persistDraft(next.value);
    requestAnimationFrame(() => {
      const el = textareaRef.current;
      if (el) {
        el.focus();
        el.setSelectionRange(next.cursorPosition, next.cursorPosition);
        setCaretPosition(next.cursorPosition);
      }
      adjustHeight();
    });
  }, [adjustHeight, persistDraft, setPlanMode, slashTrigger, value]);

  const getSlashOptionTitle = useCallback((option: SlashCommandOption) => {
    const key = option.kind === "command" ? commonSlashCommandKey(option.name, "title") : null;
    return key ? t(key) : option.title;
  }, [t]);

  const getSlashOptionDescription = useCallback((option: SlashCommandOption) => {
    const key = option.kind === "command" ? commonSlashCommandKey(option.name, "description") : null;
    return key ? t(key) : option.description;
  }, [t]);

  const getSlashSourceLabel = useCallback((option: SlashCommandOption) => {
    if (option.kind === "skill") {
      return option.sourceLabel === "Built-in skill"
        ? t("chat.slashCommandBuiltInSkill")
        : t("chat.slashCommandUserSkill");
    }
    if (option.kind === "workflow") return t("chat.slashCommandKindWorkflow");
    return t("chat.slashCommandKindCommand");
  }, [t]);

  const getSlashTabLabel = useCallback((tab: SlashCommandTab) => {
    if (tab === "command") return t("chat.slashCommandTabCommands");
    if (tab === "skill") return t("chat.slashCommandTabSkills");
    if (tab === "workflow") return t("chat.slashCommandTabWorkflows");
    return t("chat.slashCommandTabAll");
  }, [t]);

  const clearDraft = useCallback(() => {
    clearChatInputDraft(draftKey);
    draftsRef.current[draftKey] = { value: "", attachments: [] };
    resetInputHistoryNavigation();
    setValue("");
    setAttachments([]);
    setDismissedSlashToken(null);
    setCaretPosition(0);
    setTimeout(() => {
      if (textareaRef.current) {
        textareaRef.current.style.height = "auto";
      }
    }, 0);
  }, [draftKey, resetInputHistoryNavigation]);

  const handleSend = useCallback(() => {
    if (inputLocked) return;
    const trimmed = value.trim();
    if (!trimmed && attachments.length === 0) return;
    if (isStreaming && (!trimmed || attachments.length > 0)) {
      toast.error(t("chat.attachmentWhileRunning"));
      return;
    }
    const slashResolution = trimmed ? resolveSlashCommandMessage(trimmed, slashOptions) : null;
    if (slashResolution?.localAction === "openWorkflows") {
      setWorkflowCatalogOpen(true);
      const nextValue = slashResolution.message;
      setValue(nextValue);
      persistDraft(nextValue);
      requestAnimationFrame(() => textareaRef.current?.focus());
      return;
    }
    if (slashResolution?.localAction === "compact") {
      if (attachments.length === 0 && onCompact && slashResolution.message.length === 0) {
        onCompact();
        clearDraft();
        return;
      }
      toast.error(t("chat.compactMustBeAlone"));
      return;
    }
    if (slashResolution?.executionMode === "plan" && slashResolution.message.length === 0 && attachments.length === 0) {
      setPlanMode(true);
      clearDraft();
      return;
    }

    const outgoingMessage = slashResolution && !slashResolution.localAction
      ? (slashResolution.displayMessage || trimmed || slashResolution.message)
      : (slashResolution?.message || trimmed || t("chat.imageMessage"));
    const planModeArtifact: ArtifactPayload | null = effectivePlanModeEnabled
      ? {
          kind: "executionMode",
          mode: "plan",
          source: "toggle",
        }
      : null;
    const executionMode = slashResolution?.executionMode ?? (effectivePlanModeEnabled ? "plan" : undefined);
    const slashArtifact = slashResolution
      ? {
          ...slashResolution.artifact,
          ...(slashResolution.message !== outgoingMessage
            ? { [LLM_CONTEXT_CONTENT_ARTIFACT_KEY]: slashResolution.message }
            : {}),
        }
      : null;
    const baseUserArtifacts = slashArtifact ?? planModeArtifact;
    const activeGoal = !slashResolution && activeGoalContext?.status === "active"
      ? activeGoalContext
      : null;
    const goalContextContent = activeGoal
      ? buildGoalContinuationLlmContext(activeGoal, outgoingMessage)
      : null;
    const userArtifacts = activeGoal && goalContextContent
      ? mergeGoalContextArtifact(baseUserArtifacts, activeGoal, goalContextContent)
      : baseUserArtifacts;
    const sendOptions = slashResolution || executionMode || userArtifacts
      ? {
          skillIds: slashResolution?.skillIds,
          userArtifacts,
          executionMode,
        }
      : undefined;
    if (executionMode === "plan") {
      setPlanMode(true);
    }
    onSend(
      outgoingMessage,
      attachments.length > 0 ? attachments : undefined,
      sendOptions,
    );
    clearDraft();
  }, [activeGoalContext, attachments, clearDraft, effectivePlanModeEnabled, inputLocked, isStreaming, onCompact, onSend, persistDraft, setPlanMode, slashOptions, t, value]);

  const handleKeyDown = useCallback(
    (e: React.KeyboardEvent) => {
      if (slashMenuOpen && slashTrigger) {
        if (e.key === "ArrowDown") {
          e.preventDefault();
          setSlashSelectedIndex((index) => Math.min(index + 1, Math.max(0, visibleSlashMatches.length - 1)));
          return;
        }
        if (e.key === "ArrowUp") {
          e.preventDefault();
          setSlashSelectedIndex((index) => Math.max(0, index - 1));
          return;
        }
        if ((e.key === "ArrowLeft" || e.key === "ArrowRight") && slashEnabledTabs.length > 1) {
          e.preventDefault();
          const direction = e.key === "ArrowRight" ? 1 : -1;
          const currentIndex = Math.max(0, slashEnabledTabs.indexOf(slashActiveTab));
          const nextIndex = (currentIndex + direction + slashEnabledTabs.length) % slashEnabledTabs.length;
          setSlashActiveTab(slashEnabledTabs[nextIndex]);
          setSlashSelectedIndex(0);
          return;
        }
        if ((e.key === "Enter" || e.key === "Tab") && activeSlashOption) {
          e.preventDefault();
          applySlashOption(activeSlashOption);
          return;
        }
        if (e.key === "Escape") {
          e.preventDefault();
          setDismissedSlashToken(slashTrigger.token);
          return;
        }
      }
      if (e.key === "Enter" && !e.shiftKey) {
        e.preventDefault();
        if (!inputLocked) {
          handleSend();
        }
        return;
      }
      if (
        (e.key === "ArrowUp" || e.key === "ArrowDown") &&
        !e.altKey &&
        !e.ctrlKey &&
        !e.metaKey &&
        !(e.nativeEvent as KeyboardEvent).isComposing
      ) {
        const navigated = navigateInputHistory(e.key === "ArrowUp" ? "up" : "down");
        if (navigated) {
          e.preventDefault();
        }
      }
    },
    [
      activeSlashOption,
      applySlashOption,
      handleSend,
      inputLocked,
      navigateInputHistory,
      slashActiveTab,
      slashEnabledTabs,
      slashMenuOpen,
      slashTrigger,
      visibleSlashMatches.length,
    ],
  );

  const addAttachmentFromDataUrl = useCallback(
    (dataUrl: string, name: string): boolean => {
      const match = dataUrl.match(/^data:([^;]+);base64,(.+)$/);
      if (!match) return false;
      const [, mediaType, base64Data] = match;
      const allowedMediaType = getAllowedAttachmentMediaType(mediaType, name);
      if (!allowedMediaType) return false;
      setAttachments((prev) => {
        const next = [
          ...prev,
          { base64Data, mediaType: allowedMediaType, originalName: name },
        ];
        persistDraft(value, next);
        return next;
      });
      return true;
    },
    [persistDraft, value],
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
    setAttachments((prev) => {
      const next = prev.filter((_, i) => i !== index);
      persistDraft(value, next);
      return next;
    });
  }, [persistDraft, value]);

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

      const imageFiles = collectPastedImageFiles(clipboardData);
      if (imageFiles.length > 0) {
        e.preventDefault();
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

      const dataUrlImage = getPastedImageDataUrl(clipboardData);
      if (dataUrlImage) {
        if (addAttachmentFromDataUrl(dataUrlImage.dataUrl, dataUrlImage.name)) {
          e.preventDefault();
        }
      }
    },
    [addAttachment, addAttachmentFromDataUrl, attachmentLocked, t],
  );

  const applyWorkflowTemplate = useCallback((template: WorkflowCatalogTemplate) => {
    setValue((currentValue) => {
      const current = currentValue.trim();
      const batchGoal = current ? `${template.promptTemplate.trimEnd()}\n\n${current}` : undefined;
      const nextValue = buildWorkflowBatchPrompt(template, batchGoal);
      persistDraft(nextValue);
      return nextValue;
    });
    setWorkflowCatalogOpen(false);
    requestAnimationFrame(() => {
      textareaRef.current?.focus();
      adjustHeight();
    });
  }, [adjustHeight, persistDraft]);

  const modeIndicatorStyle = {
    width: "calc(50% + 0.25rem)",
    transform: effectivePlanModeEnabled ? "translateX(0)" : "translateX(calc(100% - 0.5rem))",
    borderRadius: effectivePlanModeEnabled ? "999px 2px 2px 999px" : "2px 999px 999px 2px",
    clipPath: effectivePlanModeEnabled
      ? "polygon(0 0, 100% 0, calc(100% - 0.5rem) 100%, 0 100%)"
      : "polygon(0.5rem 0, 100% 0, 100% 100%, 0 100%)",
  };

  const modeSegment = (
    <div className="flex h-7 items-center pl-1">
      <div
        data-testid="chat-mode-segment"
        className={`relative grid h-7 w-[8.75rem] shrink-0 grid-cols-2 overflow-hidden rounded-full border p-px text-[10px] font-semibold shadow-sm transition-colors duration-200 ${
          effectivePlanModeEnabled
            ? "border-accent/35 bg-accent/10 text-text-secondary"
            : "border-border/70 bg-surface-0/95 text-text-secondary"
        }`}
        role="group"
        aria-label="Message mode"
      >
        <span
          aria-hidden="true"
          data-testid="chat-mode-active-indicator"
          style={modeIndicatorStyle}
          className={`absolute bottom-px left-px top-px border shadow-sm transition-all duration-200 ease-out ${
            effectivePlanModeEnabled
              ? "border-accent/35 bg-accent text-on-accent shadow-accent/20"
              : "border-border/70 bg-surface-0 text-text-primary"
          }`}
        />
        <button
          type="button"
          data-testid="chat-plan-mode"
          onClick={() => setPlanMode(true)}
          disabled={attachmentLocked}
          aria-pressed={effectivePlanModeEnabled}
          className={`relative z-10 col-start-1 row-start-1 flex min-w-0 items-center justify-center rounded-full pl-2 pr-3 transition-colors duration-200 disabled:pointer-events-none disabled:opacity-45 ${
            effectivePlanModeEnabled ? "text-on-accent" : "text-text-tertiary hover:text-text-primary"
          }`}
        >
          <span className="truncate">{t("chat.planLabel")}</span>
        </button>
        <span
          aria-hidden="true"
          data-testid="chat-mode-divider"
          className={`pointer-events-none absolute inset-y-px left-1/2 z-20 flex w-3 -translate-x-1/2 items-center justify-center text-[10px] font-medium leading-none transition-colors duration-200 ${
            effectivePlanModeEnabled ? "text-accent/70" : "text-text-tertiary/70"
          }`}
        >
          /
        </span>
        <button
          type="button"
          data-testid="chat-normal-mode"
          onClick={() => setPlanMode(false)}
          disabled={attachmentLocked}
          aria-pressed={!effectivePlanModeEnabled}
          className={`relative z-10 col-start-2 row-start-1 flex min-w-0 items-center justify-center rounded-full pl-3 pr-2 transition-colors duration-200 disabled:pointer-events-none disabled:opacity-45 ${
            effectivePlanModeEnabled ? "text-text-tertiary hover:text-text-primary" : "text-text-primary"
          }`}
        >
          <span className="truncate">{t("chat.normalLabel")}</span>
        </button>
      </div>
    </div>
  );
  const planModeBanner = effectivePlanModeEnabled ? (
    <div
      data-testid="chat-plan-mode-banner"
      className="flex min-w-0 items-center gap-2 rounded-lg border border-accent/25 bg-accent/10 px-2.5 py-2 text-xs text-text-secondary"
    >
      <span className="flex h-6 w-6 shrink-0 items-center justify-center rounded-md border border-accent/25 bg-surface-0/70 text-accent">
        <BrainCircuit className="h-3.5 w-3.5" />
      </span>
      <div className="min-w-0 flex-1">
        <div className="truncate font-medium text-text-primary">Plan mode</div>
        <div className="truncate text-[11px] text-text-tertiary">
          Read-only proposal. Approve the plan card to switch into implementation.
        </div>
      </div>
      <button
        type="button"
        onClick={() => setPlanMode(false)}
        className="flex h-6 w-6 shrink-0 items-center justify-center rounded-md text-text-tertiary transition-colors hover:bg-surface-0 hover:text-text-primary"
        aria-label={t("common.close")}
      >
        <X className="h-3.5 w-3.5" />
      </button>
    </div>
  ) : null;

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

      {slashMenuOpen && slashTrigger && (
        <div
          data-testid="slash-command-menu"
          className="absolute bottom-full left-4 z-40 mb-2 w-[min(34rem,calc(100vw-2rem))] overflow-hidden rounded-lg border border-border/70 bg-surface-0 shadow-2xl shadow-black/30 ring-1 ring-white/[0.04]"
        >
          <div className="border-b border-border/60 px-2.5 py-1.5">
            <div className="flex min-h-7 items-center gap-2">
              <Command className="h-3.5 w-3.5 shrink-0 text-accent" />
              <div className="min-w-0">
                <div className="text-xs font-medium text-text-primary">{t("chat.slashCommands")}</div>
                <div className="truncate text-[10px] text-text-tertiary">/{slashTrigger.query}</div>
              </div>
              <div
                className="ml-auto rounded-md border border-border/60 bg-surface-1 px-1.5 py-0.5 text-[10px] uppercase text-text-tertiary"
                aria-label={t("chat.slashCommandCount", { count: String(visibleSlashMatches.length) })}
              >
                {visibleSlashMatches.length === 0 ? t("chat.slashCommandNoMatch") : visibleSlashMatches.length}
              </div>
            </div>

            <div className="mt-1.5 grid grid-cols-4 gap-1 rounded-md border border-border/50 bg-surface-1 p-0.5" role="tablist">
              {SLASH_COMMAND_TABS.map((tab) => {
                const selected = tab === slashActiveTab;
                const count = slashTabCounts[tab];
                const disabledTab = tab !== "all" && count === 0;
                return (
                  <button
                    key={tab}
                    type="button"
                    data-testid={`slash-command-tab-${tab}`}
                    role="tab"
                    aria-selected={selected}
                    disabled={disabledTab}
                    onMouseDown={(event) => event.preventDefault()}
                    onClick={() => {
                      setSlashActiveTab(tab);
                      setSlashSelectedIndex(0);
                    }}
                    className={`flex min-w-0 items-center justify-center gap-1 rounded px-1.5 py-0.5 text-[10px] font-medium transition-colors ${
                      selected
                        ? "bg-surface-0 text-text-primary shadow-sm ring-1 ring-border/60"
                        : "text-text-tertiary hover:bg-surface-2 hover:text-text-primary disabled:cursor-not-allowed disabled:opacity-35"
                    }`}
                  >
                    <span className="truncate">{getSlashTabLabel(tab)}</span>
                    <span className="shrink-0 tabular-nums text-[10px] opacity-70">{count}</span>
                  </button>
                );
              })}
            </div>
          </div>

          {visibleSlashMatches.length > 0 ? (
            <>
              <div
                data-testid="slash-command-list"
                className="max-h-52 overflow-y-auto p-1"
                role="listbox"
                aria-label={t("chat.slashCommands")}
              >
                {visibleSlashMatches.map((option, index) => {
                const active = index === activeSlashIndex;
                const Icon = option.kind === "skill" ? BrainCircuit : option.kind === "workflow" ? Workflow : Command;
                const optionTitle = getSlashOptionTitle(option);
                const optionDescription = getSlashOptionDescription(option);
                return (
                  <button
                    key={option.id}
                    ref={(node) => {
                      slashOptionRefs.current[index] = node;
                    }}
                    type="button"
                    role="option"
                    aria-selected={active}
                    data-testid={`slash-command-option-${option.name}`}
                    onMouseDown={(event) => event.preventDefault()}
                    onClick={() => applySlashOption(option)}
                    className={`grid w-full grid-cols-[1.5rem_minmax(0,1fr)_auto] items-center gap-1.5 rounded-md px-1.5 py-1 text-left transition-colors ${
                      active
                        ? "bg-accent-subtle text-text-primary ring-1 ring-accent/25"
                        : "text-text-secondary hover:bg-surface-1 hover:text-text-primary"
                    }`}
                  >
                    <span
                      className={`flex h-6 w-6 items-center justify-center rounded-md border ${
                        active
                          ? "border-accent/35 bg-accent/10 text-accent"
                          : "border-border/60 bg-surface-1 text-text-tertiary"
                      }`}
                    >
                      <Icon className="h-3.5 w-3.5" />
                    </span>
                    <span className="min-w-0">
                      <span className="flex min-w-0 items-baseline gap-2">
                        <span className="shrink-0 font-mono text-[12px] text-accent">/{option.name}</span>
                        <span className="truncate text-xs font-medium text-text-primary">{optionTitle}</span>
                      </span>
                      <span className="mt-0.5 block truncate text-[10px] leading-3 text-text-tertiary">
                        {optionDescription}
                      </span>
                    </span>
                    <span className="hidden max-w-24 truncate rounded-md border border-border/50 bg-surface-1 px-1.5 py-0.5 text-[10px] text-text-tertiary sm:block">
                      {getSlashSourceLabel(option)}
                    </span>
                  </button>
                );
              })}
              </div>

              {activeSlashOption && (
                <div className="hidden border-t border-border/60 px-2.5 py-1.5 sm:block">
                  <div className="flex min-w-0 items-center gap-2 text-[11px]">
                    <span className="shrink-0 font-mono text-accent">/{activeSlashOption.name}</span>
                    <span className="shrink-0 rounded border border-border/50 bg-surface-1 px-1.5 py-0.5 text-[10px] text-text-tertiary">
                      {getSlashSourceLabel(activeSlashOption)}
                    </span>
                    <span className="truncate font-medium text-text-primary">
                      {getSlashOptionTitle(activeSlashOption)}
                    </span>
                  </div>
                  <div className="mt-0.5 truncate text-[11px] text-text-tertiary">
                    {getSlashOptionDescription(activeSlashOption)}
                  </div>
                </div>
              )}
            </>
          ) : (
            <div className="px-3 py-4 text-center text-xs text-text-tertiary">
              {t("chat.slashCommandNoResults")}
            </div>
          )}
        </div>
      )}

      {workflowCatalogOpen && !slashMenuOpen && (
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

      <div className="space-y-2">
        {modeSegment}
        {planModeBanner}

        <div
          className={`overflow-hidden rounded-xl border bg-surface-0 shadow-[0_12px_32px_rgba(0,0,0,0.16)] ring-1 ring-white/[0.03] transition-colors duration-fast focus-within:border-accent/55 focus-within:ring-accent/20 ${
            effectivePlanModeEnabled ? "border-accent/35" : "border-border/80"
          }`}
        >
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
          onChange={(e) => {
            const nextValue = e.target.value;
            resetInputHistoryNavigation();
            setValue(nextValue);
            persistDraft(nextValue);
            setCaretPosition(e.target.selectionStart);
            setDismissedSlashToken(null);
          }}
          onKeyDown={handleKeyDown}
          onKeyUp={updateCaretFromTextarea}
          onClick={updateCaretFromTextarea}
          onSelect={updateCaretFromTextarea}
          onPaste={handlePaste}
          placeholder={isCompacting ? `${t("chat.compacting")} (>_<)` : t("chat.placeholder")}
          disabled={inputLocked}
          rows={1}
          className="block min-h-24 w-full resize-none overflow-y-auto bg-transparent px-4 pb-3 pt-3.5 text-sm leading-6 text-text-primary placeholder:text-text-tertiary outline-none disabled:pointer-events-none disabled:opacity-40"
        />

        <div className="flex min-h-11 items-center justify-between gap-3 border-t border-border/35 px-2.5 py-2">
          <div className="flex min-w-0 flex-1 items-center gap-1.5 overflow-x-auto overflow-y-hidden">
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

            {sessionControls}

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
              onTranscript={(text) => {
                setValue((prev) => {
                  const nextValue = prev + (prev ? " " : "") + text;
                  persistDraft(nextValue);
                  return nextValue;
                });
              }}
              disabled={attachmentLocked}
            />

            <EmojiPicker
              onEmojiSelect={(emoji) => {
                setValue((prev) => {
                  const nextValue = prev + emoji;
                  persistDraft(nextValue);
                  return nextValue;
                });
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
    </div>
  );
}
