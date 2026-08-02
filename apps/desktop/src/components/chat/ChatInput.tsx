import { useState, useRef, useCallback, useEffect, useMemo, type ReactNode } from "react";
import {
  NexaPopover,
  NexaPopoverAnchor,
  NexaPopoverContent,
  NexaPopoverTrigger,
  NexaSelect,
} from "../ui/overlay";
import { AnimatePresence, motion, useReducedMotion } from "framer-motion";
import { ArrowUp, Square, Paperclip, X, FileText, Workflow, ChevronDown, ArchiveRestore, Loader2, Command, BrainCircuit, Sparkles, CircleDollarSign, Timer, Users, ShieldCheck, TriangleAlert } from "lucide-react";
import { toast } from "sonner";
import { useTranslation, type TranslationKey } from "../../i18n";
import type { ArtifactPayload, Conversation, ImageAttachment } from "../../types/conversation";
import type { Skill } from "../../types/extensions";
import type {
  AgentCollaborationMode,
  AgentExecutionMode,
  AgentPowerMode,
  CustomOrchestrationOptions,
  MoaPresetId,
  OrchestrationProfile,
  WorkflowCatalogTemplate,
} from "../../lib/api";
import * as api from "../../lib/api";
import {
  buildSlashCommandOptions,
  getMatchingSlashCommands,
  getSlashCommandTrigger,
  resolveSlashCommandMessage,
  resolveSlashCommandSelection,
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
import { Modal } from "../ui/Modal";

const LLM_CONTEXT_CONTENT_ARTIFACT_KEY = "llmContextContent";

export interface ChatInputSendOptions {
  skillIds?: string[];
  userArtifacts?: ArtifactPayload | null;
  executionMode?: AgentExecutionMode;
  powerMode?: AgentPowerMode;
  collaborationMode?: AgentCollaborationMode;
  moaPreset?: MoaPresetId;
  orchestrationProfile?: OrchestrationProfile;
  customOrchestration?: CustomOrchestrationOptions | null;
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
  prefillKey?: number;
  onCompact?: () => void;
  isCompacting?: boolean;
  planModeEnabled?: boolean;
  onPlanModeChange?: (enabled: boolean) => void;
  activeGoalContext?: ActiveGoalContext | null;
  contextIndicator?: ReactNode;
}

interface ChatDraftState {
  value: string;
  attachments: ImageAttachment[];
  activeSlashCommandId: string | null;
}

interface StoredChatDraftState {
  value: string;
  activeSlashCommandId?: string | null;
  updatedAt: number;
}

type SlashCommandTab = "all" | SlashCommandKind;

const NEW_CONVERSATION_DRAFT_KEY = "__new__";
const CHAT_INPUT_DRAFT_STORAGE_KEY = "chat-input-drafts-v1";
const CHAT_POWER_MODE_STORAGE_PREFIX = "chat-agent-power-mode-v1";
const CHAT_NEXUS_ACKNOWLEDGED_STORAGE_KEY = "chat-nexus-mode-acknowledged-v1";
const CHAT_ORCHESTRATION_STORAGE_PREFIX = "chat-orchestration-policy-v1";
const MAX_STORED_CHAT_INPUT_DRAFTS = 100;
const MAX_INPUT_HISTORY_ITEMS = 100;
const chatInputDrafts: Record<string, ChatDraftState> = {};

function readStoredPowerMode(key: string): AgentPowerMode | null {
  try {
    const value = localStorage.getItem(`${CHAT_POWER_MODE_STORAGE_PREFIX}:${key}`);
    return value === "nexus" || value === "standard" ? value : null;
  } catch {
    return null;
  }
}

function persistPowerMode(key: string, mode: AgentPowerMode) {
  try {
    localStorage.setItem(`${CHAT_POWER_MODE_STORAGE_PREFIX}:${key}`, mode);
  } catch {
    // Storage can be unavailable in privacy-restricted browser contexts.
  }
}

function hasAcknowledgedNexusMode(): boolean {
  try {
    return localStorage.getItem(CHAT_NEXUS_ACKNOWLEDGED_STORAGE_KEY) === "true";
  } catch {
    return false;
  }
}

function acknowledgeNexusMode() {
  try {
    localStorage.setItem(CHAT_NEXUS_ACKNOWLEDGED_STORAGE_KEY, "true");
  } catch {
    // Keep activation functional when persistent storage is unavailable.
  }
}

interface StoredOrchestrationPolicy {
  collaborationMode: AgentCollaborationMode;
  moaPreset: MoaPresetId;
  orchestrationProfile: OrchestrationProfile;
  customOrchestration: CustomOrchestrationOptions;
}

const DEFAULT_CUSTOM_ORCHESTRATION: CustomOrchestrationOptions = {
  maxIterations: 32,
  maxParallel: 3,
  maxCallsPerTurn: 6,
  delegatedTokenBudget: 48_000,
  verificationReservePercent: 25,
  retryLimit: 2,
  minEvidenceSources: 2,
};

function readStoredOrchestrationPolicy(key: string): StoredOrchestrationPolicy {
  try {
    const parsed = JSON.parse(
      localStorage.getItem(`${CHAT_ORCHESTRATION_STORAGE_PREFIX}:${key}`) ?? "null",
    ) as Partial<StoredOrchestrationPolicy> | null;
    const collaborationMode = parsed?.collaborationMode === "mixtureOfAgents"
      ? "mixtureOfAgents"
      : "direct";
    const moaPreset = ["fastReview", "deepResearch", "crossModelCodeReview", "custom"].includes(
      parsed?.moaPreset ?? "",
    ) ? parsed!.moaPreset as MoaPresetId : "fastReview";
    const orchestrationProfile = ["balanced", "deep", "codeUltra", "researchUltra", "custom"].includes(
      parsed?.orchestrationProfile ?? "",
    ) ? parsed!.orchestrationProfile as OrchestrationProfile : "balanced";
    return {
      collaborationMode,
      moaPreset,
      orchestrationProfile,
      customOrchestration: { ...DEFAULT_CUSTOM_ORCHESTRATION, ...parsed?.customOrchestration },
    };
  } catch {
    return {
      collaborationMode: "direct",
      moaPreset: "fastReview",
      orchestrationProfile: "balanced",
      customOrchestration: { ...DEFAULT_CUSTOM_ORCHESTRATION },
    };
  }
}

function persistOrchestrationPolicy(key: string, policy: StoredOrchestrationPolicy) {
  try {
    localStorage.setItem(`${CHAT_ORCHESTRATION_STORAGE_PREFIX}:${key}`, JSON.stringify(policy));
  } catch {
    // Storage can be unavailable in privacy-restricted browser contexts.
  }
}

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
    activeSlashCommandId: draft.activeSlashCommandId,
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
      drafts[key] = {
        value: row.value,
        activeSlashCommandId: typeof row.activeSlashCommandId === "string" ? row.activeSlashCommandId : null,
        updatedAt,
      };
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
  const draft = {
    value: stored?.value ?? "",
    attachments: [],
    activeSlashCommandId: stored?.activeSlashCommandId ?? null,
  };
  chatInputDrafts[draftKey] = cloneDraftState(draft);
  return draft;
}

function persistChatInputDraft(draftKey: string, draft: ChatDraftState) {
  chatInputDrafts[draftKey] = cloneDraftState(draft);

  const storedDrafts = readStoredChatInputDrafts();
  if (draft.value.length > 0 || draft.activeSlashCommandId) {
    storedDrafts[draftKey] = {
      value: draft.value,
      activeSlashCommandId: draft.activeSlashCommandId,
      updatedAt: Date.now(),
    };
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
  prefillKey,
  onCompact,
  isCompacting = false,
  planModeEnabled,
  onPlanModeChange,
  activeGoalContext,
  contextIndicator,
}: ChatInputProps) {
  const { t } = useTranslation();
  const shouldReduceMotion = useReducedMotion();
  const draftKey = conversationId ?? NEW_CONVERSATION_DRAFT_KEY;
  const initialDraftRef = useRef<ChatDraftState | null>(null);
  if (initialDraftRef.current === null) {
    initialDraftRef.current = readChatInputDraft(draftKey);
  }
  const [value, setValue] = useState(() => initialDraftRef.current?.value ?? "");
  const [attachments, setAttachments] = useState<ImageAttachment[]>(() => (
    initialDraftRef.current?.attachments ?? []
  ));
  const [activeSlashCommandId, setActiveSlashCommandId] = useState<string | null>(
    () => initialDraftRef.current?.activeSlashCommandId ?? null,
  );
  const [previewAttachment, setPreviewAttachment] = useState<ImageAttachment | null>(null);
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
  const [powerMode, setPowerModeState] = useState<AgentPowerMode>(
    () => readStoredPowerMode(draftKey) ?? "standard",
  );
  const [collaborationMode, setCollaborationModeState] = useState<AgentCollaborationMode>(
    () => readStoredOrchestrationPolicy(draftKey).collaborationMode,
  );
  const [moaPreset, setMoaPresetState] = useState<MoaPresetId>(
    () => readStoredOrchestrationPolicy(draftKey).moaPreset,
  );
  const [orchestrationProfile, setOrchestrationProfileState] = useState<OrchestrationProfile>(
    () => readStoredOrchestrationPolicy(draftKey).orchestrationProfile,
  );
  const [customOrchestration, setCustomOrchestrationState] = useState<CustomOrchestrationOptions>(
    () => readStoredOrchestrationPolicy(draftKey).customOrchestration,
  );
  const [nexusDialogOpen, setNexusDialogOpen] = useState(false);
  const [nexusActivationVisible, setNexusActivationVisible] = useState(false);
  const textareaRef = useRef<HTMLTextAreaElement>(null);
  const fileInputRef = useRef<HTMLInputElement>(null);
  const slashOptionRefs = useRef<Array<HTMLButtonElement | null>>([]);
  const dragCounterRef = useRef(0);
  const draftsRef = useRef<Record<string, ChatDraftState>>(
    initialDraftRef.current ? { [draftKey]: cloneDraftState(initialDraftRef.current) } : {},
  );
  const historyDraftRef = useRef<{ value: string; cursor: number } | null>(null);
  const previousPowerModeKeyRef = useRef(draftKey);
  // Compaction only locks actions that mutate conversation history. The draft
  // remains fully editable so the user can keep typing while the checkpoint is
  // being built, then send as soon as compaction completes.
  const inputLocked = disabled;
  const sendLocked = inputLocked || isCompacting;
  const attachmentLocked = sendLocked || isStreaming;
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

  const setPowerMode = useCallback((mode: AgentPowerMode) => {
    setPowerModeState(mode);
    persistPowerMode(draftKey, mode);
  }, [draftKey]);

  const persistRuntimePolicy = useCallback((next: Partial<StoredOrchestrationPolicy>) => {
    const policy: StoredOrchestrationPolicy = {
      collaborationMode: next.collaborationMode ?? collaborationMode,
      moaPreset: next.moaPreset ?? moaPreset,
      orchestrationProfile: next.orchestrationProfile ?? orchestrationProfile,
      customOrchestration: next.customOrchestration ?? customOrchestration,
    };
    setCollaborationModeState(policy.collaborationMode);
    setMoaPresetState(policy.moaPreset);
    setOrchestrationProfileState(policy.orchestrationProfile);
    setCustomOrchestrationState(policy.customOrchestration);
    persistOrchestrationPolicy(draftKey, policy);
  }, [collaborationMode, customOrchestration, draftKey, moaPreset, orchestrationProfile]);

  const activateNexusMode = useCallback(() => {
    setPowerMode("nexus");
    acknowledgeNexusMode();
    setNexusDialogOpen(false);
    if (!shouldReduceMotion) {
      setNexusActivationVisible(true);
    }
  }, [setPowerMode, shouldReduceMotion]);

  useEffect(() => {
    const previousKey = previousPowerModeKeyRef.current;
    let storedMode = readStoredPowerMode(draftKey);
    if (
      storedMode === null
      && previousKey === NEW_CONVERSATION_DRAFT_KEY
      && draftKey !== NEW_CONVERSATION_DRAFT_KEY
      && powerMode === "nexus"
    ) {
      storedMode = "nexus";
      persistPowerMode(draftKey, storedMode);
    }
    setPowerModeState(storedMode ?? "standard");
    let storedPolicy = readStoredOrchestrationPolicy(draftKey);
    if (
      previousKey === NEW_CONVERSATION_DRAFT_KEY
      && draftKey !== NEW_CONVERSATION_DRAFT_KEY
      && storedPolicy.collaborationMode === "direct"
      && storedPolicy.orchestrationProfile === "balanced"
      && (collaborationMode === "mixtureOfAgents" || orchestrationProfile !== "balanced")
    ) {
      storedPolicy = {
        collaborationMode,
        moaPreset,
        orchestrationProfile,
        customOrchestration,
      };
      persistOrchestrationPolicy(draftKey, storedPolicy);
    }
    setCollaborationModeState(storedPolicy.collaborationMode);
    setMoaPresetState(storedPolicy.moaPreset);
    setOrchestrationProfileState(storedPolicy.orchestrationProfile);
    setCustomOrchestrationState(storedPolicy.customOrchestration);
    previousPowerModeKeyRef.current = draftKey;
  }, [draftKey]);

  const persistDraft = useCallback((
    nextValue: string,
    nextAttachments: ImageAttachment[] = attachments,
    nextSlashCommandId: string | null = activeSlashCommandId,
  ) => {
    const draft = {
      value: nextValue,
      attachments: nextAttachments,
      activeSlashCommandId: nextSlashCommandId,
    };
    draftsRef.current[draftKey] = cloneDraftState(draft);
    persistChatInputDraft(draftKey, draft);
  }, [activeSlashCommandId, attachments, draftKey]);

  useEffect(() => {
    const draft = draftsRef.current[draftKey] ?? readChatInputDraft(draftKey);
    draftsRef.current[draftKey] = cloneDraftState(draft);
    resetInputHistoryNavigation();
    setLoadedDraftKey(draftKey);
    setValue(draft.value);
    setAttachments(draft.attachments);
    setActiveSlashCommandId(draft.activeSlashCommandId);
    setPreviewAttachment(null);
    setTimeout(() => {
      if (textareaRef.current) {
        textareaRef.current.style.height = "auto";
      }
    }, 0);
  }, [draftKey, resetInputHistoryNavigation]);

  useEffect(() => {
    if (loadedDraftKey !== draftKey) return;
    persistDraft(value, attachments, activeSlashCommandId);
  }, [activeSlashCommandId, attachments, draftKey, loadedDraftKey, persistDraft, value]);

  // Accept prefilled text from outside (e.g. suggestion cards)
  useEffect(() => {
    if (prefillText != null && prefillText !== "") {
      resetInputHistoryNavigation();
      setValue(prefillText);
      persistDraft(prefillText);
      setTimeout(() => textareaRef.current?.focus(), 0);
    }
  }, [persistDraft, prefillKey, prefillText, resetInputHistoryNavigation]);

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
  const activeSlashCommand = useMemo(
    () => slashOptions.find((option) => option.id === activeSlashCommandId) ?? null,
    [activeSlashCommandId, slashOptions],
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
      setActiveSlashCommandId(null);
      persistDraft(nextValue, attachments, null);
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
      setActiveSlashCommandId(null);
      persistDraft(nextValue, attachments, null);
      setPlanMode(true);
      requestAnimationFrame(() => {
        textareaRef.current?.focus();
        setCaretPosition(textareaRef.current?.selectionStart ?? nextValue.length);
        adjustHeight();
      });
      return;
    }

    const nextValue = `${value.slice(0, slashTrigger.start)}${value.slice(slashTrigger.end)}`.trimStart();
    const nextCursor = Math.min(slashTrigger.start, nextValue.length);
    setValue(nextValue);
    setActiveSlashCommandId(option.id);
    persistDraft(nextValue, attachments, option.id);
    requestAnimationFrame(() => {
      const el = textareaRef.current;
      if (el) {
        el.focus();
        el.setSelectionRange(nextCursor, nextCursor);
        setCaretPosition(nextCursor);
      }
      adjustHeight();
    });
  }, [adjustHeight, attachments, persistDraft, setPlanMode, slashTrigger, value]);

  const removeActiveSlashCommand = useCallback(() => {
    setActiveSlashCommandId(null);
    persistDraft(value, attachments, null);
    requestAnimationFrame(() => textareaRef.current?.focus());
  }, [attachments, persistDraft, value]);

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
    draftsRef.current[draftKey] = { value: "", attachments: [], activeSlashCommandId: null };
    resetInputHistoryNavigation();
    setValue("");
    setAttachments([]);
    setActiveSlashCommandId(null);
    setPreviewAttachment(null);
    setDismissedSlashToken(null);
    setCaretPosition(0);
    setTimeout(() => {
      if (textareaRef.current) {
        textareaRef.current.style.height = "auto";
      }
    }, 0);
  }, [draftKey, resetInputHistoryNavigation]);

  const handleSend = useCallback(() => {
    if (sendLocked) return;
    const trimmed = value.trim();
    if (!trimmed && attachments.length === 0 && !activeSlashCommand) return;
    if (isStreaming && (!trimmed || attachments.length > 0)) {
      toast.error(t("chat.attachmentWhileRunning"));
      return;
    }
    const slashResolution = activeSlashCommand
      ? resolveSlashCommandSelection(activeSlashCommand, trimmed)
      : (trimmed ? resolveSlashCommandMessage(trimmed, slashOptions) : null);
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
    const sendOptions = {
      skillIds: slashResolution?.skillIds,
      userArtifacts,
      executionMode,
      powerMode,
      collaborationMode,
      moaPreset,
      orchestrationProfile,
      customOrchestration: orchestrationProfile === "custom" ? customOrchestration : null,
    };
    if (executionMode === "plan") {
      setPlanMode(true);
    }
    onSend(
      outgoingMessage,
      attachments.length > 0 ? attachments : undefined,
      sendOptions,
    );
    clearDraft();
  }, [activeGoalContext, activeSlashCommand, attachments, clearDraft, collaborationMode, customOrchestration, effectivePlanModeEnabled, isStreaming, moaPreset, onCompact, onSend, orchestrationProfile, persistDraft, powerMode, sendLocked, setPlanMode, slashOptions, t, value]);

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
        if (!sendLocked) {
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
      sendLocked,
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
      const removed = prev[index];
      const next = prev.filter((_, i) => i !== index);
      if (removed && previewAttachment === removed) {
        setPreviewAttachment(null);
      }
      persistDraft(value, next);
      return next;
    });
  }, [persistDraft, previewAttachment, value]);

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

  const nexusRiskItems: Array<{
    Icon: typeof CircleDollarSign;
    key: TranslationKey;
  }> = [
    { Icon: CircleDollarSign, key: "chat.nexusCostRisk" },
    { Icon: Timer, key: "chat.nexusLatencyRisk" },
    { Icon: Users, key: "chat.nexusParallelRisk" },
    { Icon: ShieldCheck, key: "chat.nexusQualityRisk" },
  ];

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
  const nexusModeEnabled = powerMode === "nexus";
  const nexusModeBanner = nexusModeEnabled ? (
    <div
      data-testid="chat-nexus-mode-banner"
      className="flex min-w-0 items-center gap-2 rounded-lg border border-violet-400/25 bg-linear-to-r from-violet-500/12 via-accent/8 to-transparent px-2.5 py-2 text-xs text-text-secondary"
    >
      <span className="flex h-6 w-6 shrink-0 items-center justify-center rounded-md border border-violet-400/25 bg-surface-0/70 text-violet-300">
        <Sparkles className="h-3.5 w-3.5" />
      </span>
      <div className="min-w-0 flex-1">
        <div className="truncate font-medium text-text-primary">{t("chat.nexusMode")}</div>
        <div className="truncate text-[11px] text-text-tertiary">{t("chat.nexusBannerSummary")}</div>
      </div>
      <button
        type="button"
        onClick={() => setNexusDialogOpen(true)}
        className="shrink-0 rounded-md px-2 py-1 text-[11px] font-medium text-violet-300 transition-colors hover:bg-surface-0/70 hover:text-violet-200"
      >
        {t("chat.nexusDetails")}
      </button>
      <button
        type="button"
        onClick={() => setPowerMode("standard")}
        className="flex h-6 w-6 shrink-0 items-center justify-center rounded-md text-text-tertiary transition-colors hover:bg-surface-0 hover:text-text-primary"
        aria-label={t("chat.nexusDisable")}
      >
        <X className="h-3.5 w-3.5" />
      </button>
    </div>
  ) : null;
  const moaModeEnabled = collaborationMode === "mixtureOfAgents";
  const moaModeBanner = moaModeEnabled ? (
    <div
      data-testid="chat-moa-mode-banner"
      className="flex min-w-0 items-center gap-2 rounded-lg border border-cyan-400/25 bg-cyan-500/8 px-2.5 py-2 text-xs text-text-secondary"
    >
      <span className="flex h-6 w-6 shrink-0 items-center justify-center rounded-md border border-cyan-400/25 bg-surface-0/70 text-cyan-300">
        <Users className="h-3.5 w-3.5" />
      </span>
      <div className="min-w-0 flex-1">
        <div className="truncate font-medium text-text-primary">
          {t("chat.moaMode")} · {t(`chat.moaPreset.${moaPreset}` as TranslationKey)}
        </div>
        <div className="truncate text-[11px] text-text-tertiary">{t("chat.moaBannerSummary")}</div>
      </div>
      <span className="hidden shrink-0 rounded-full border border-cyan-400/20 px-2 py-0.5 text-[10px] text-cyan-300 sm:inline">
        {nexusModeEnabled ? t("chat.moaWithNexus") : t("chat.moaIndependent")}
      </span>
      <button
        type="button"
        onClick={() => persistRuntimePolicy({ collaborationMode: "direct" })}
        className="flex h-6 w-6 shrink-0 items-center justify-center rounded-md text-text-tertiary transition-colors hover:bg-surface-0 hover:text-text-primary"
        aria-label={t("chat.moaDisable")}
      >
        <X className="h-3.5 w-3.5" />
      </button>
    </div>
  ) : null;
  const qualityProfileBanner = orchestrationProfile !== "balanced" ? (
    <div
      data-testid="chat-quality-profile-banner"
      className="rounded-lg border border-amber-400/25 bg-amber-500/8 px-2.5 py-2 text-xs text-text-secondary"
    >
      <div className="flex min-w-0 items-center gap-2">
        <span className="flex h-6 w-6 shrink-0 items-center justify-center rounded-md border border-amber-400/25 bg-surface-0/70 text-amber-300">
          <ShieldCheck className="h-3.5 w-3.5" />
        </span>
        <div className="min-w-0 flex-1">
          <div className="truncate font-medium text-text-primary">
            {t(`chat.qualityProfile.${orchestrationProfile}` as TranslationKey)}
          </div>
          <div className="truncate text-[11px] text-text-tertiary">{t("chat.qualityProfileSummary")}</div>
        </div>
        <button
          type="button"
          onClick={() => persistRuntimePolicy({ orchestrationProfile: "balanced" })}
          className="flex h-6 w-6 shrink-0 items-center justify-center rounded-md text-text-tertiary transition-colors hover:bg-surface-0 hover:text-text-primary"
          aria-label={t("chat.qualityProfileReset")}
        >
          <X className="h-3.5 w-3.5" />
        </button>
      </div>
      {orchestrationProfile === "custom" && (
        <div className="mt-2 grid grid-cols-2 gap-2 border-t border-amber-400/15 pt-2 sm:grid-cols-4">
          {([
            ["maxIterations", "chat.qualityCustomIterations", 4, 96, 1],
            ["maxParallel", "chat.qualityCustomParallel", 1, 8, 1],
            ["maxCallsPerTurn", "chat.qualityCustomCalls", 1, 24, 1],
            ["delegatedTokenBudget", "chat.qualityCustomTokenBudget", 4096, 192000, 1024],
            ["retryLimit", "chat.qualityCustomRetries", 0, 5, 1],
            ["minEvidenceSources", "chat.qualityCustomEvidence", 0, 8, 1],
            ["verificationReservePercent", "chat.qualityCustomReserve", 10, 50, 5],
          ] as const).map(([field, label, min, max, step]) => (
            <label key={field} className="min-w-0 text-[10px] text-text-tertiary">
              <span className="mb-1 block truncate">{t(label)}</span>
              <input
                data-testid={`chat-quality-custom-${field}`}
                type="number"
                min={min}
                max={max}
                step={step}
                value={customOrchestration[field] ?? min}
                onChange={(event) => {
                  const value = Math.min(max, Math.max(min, Number(event.target.value) || min));
                  persistRuntimePolicy({
                    customOrchestration: { ...customOrchestration, [field]: value },
                  });
                }}
                className="h-7 w-full rounded-md border border-border/70 bg-surface-0 px-2 text-xs text-text-primary outline-none focus:border-amber-400/45"
              />
            </label>
          ))}
        </div>
      )}
    </div>
  ) : null;

  return (
    <NexaPopover
      open={slashMenuOpen}
      onOpenChange={(open) => {
        if (!open && slashTrigger) setDismissedSlashToken(slashTrigger.token);
      }}
    >
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
        <NexaPopoverContent
          data-testid="slash-command-menu"
          side="top"
          align="start"
          collisionPadding={16}
          onOpenAutoFocus={(event) => event.preventDefault()}
          onCloseAutoFocus={(event) => event.preventDefault()}
          className="w-[min(34rem,calc(100vw-2rem))] overflow-hidden p-0"
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
        </NexaPopoverContent>
      )}

      <div className="space-y-2">
        <div className="flex min-h-8 items-center justify-between gap-2">
          {modeSegment}
          {contextIndicator}
        </div>
        {planModeBanner}
        {nexusModeBanner}
        {moaModeBanner}
        {qualityProfileBanner}

        <div
          className={`overflow-visible rounded-xl border bg-surface-0 shadow-[0_12px_32px_rgba(0,0,0,0.16)] ring-1 ring-white/[0.03] transition-colors duration-fast focus-within:border-accent/55 focus-within:ring-accent/20 ${
            effectivePlanModeEnabled ? "border-accent/35" : "border-border/80"
          }`}
        >
        <AnimatePresence initial={false}>
          {activeSlashCommand && (
            <motion.div
              data-testid="active-slash-command"
              className="flex items-center border-b border-border/35 bg-linear-to-r from-accent/10 via-accent/4 to-transparent px-3 py-2"
              initial={{ opacity: 0, height: 0, y: -6 }}
              animate={{ opacity: 1, height: 'auto', y: 0 }}
              exit={{ opacity: 0, height: 0, y: -5 }}
              transition={{ type: 'spring', stiffness: 430, damping: 34 }}
            >
              <button
                type="button"
                data-testid="remove-active-slash-command"
                onClick={removeActiveSlashCommand}
                className="group inline-flex max-w-full items-center gap-2 rounded-full border border-accent/30 bg-accent/12 py-1 pl-2.5 pr-1.5 text-xs text-accent shadow-[0_4px_14px_rgba(80,100,255,0.12)] transition hover:border-accent/55 hover:bg-accent/18"
                aria-label={t('chat.removeActiveSlashCommand', { command: getSlashOptionTitle(activeSlashCommand) })}
              >
                <Sparkles size={12} className="shrink-0" />
                <span className="truncate font-medium">/{activeSlashCommand.name}</span>
                <span className="hidden truncate text-[10px] text-text-secondary sm:inline">
                  {getSlashOptionTitle(activeSlashCommand)}
                </span>
                <span className="grid h-5 w-5 shrink-0 place-items-center rounded-full text-text-tertiary transition group-hover:bg-surface-1 group-hover:text-text-primary">
                  <X size={11} />
                </span>
              </button>
            </motion.div>
          )}
        </AnimatePresence>
        {attachments.length > 0 && (
          <div className="flex flex-wrap gap-1.5 border-b border-border/35 px-3 py-2">
            {attachments.map((att, i) => (
              <div key={i} className="relative group">
                {att.mediaType.startsWith("image/") ? (
                  <button
                    type="button"
                    data-testid="chat-attachment-thumbnail"
                    onClick={() => setPreviewAttachment(att)}
                    className="block h-10 w-10 overflow-hidden rounded-md border border-border bg-surface-2 transition-colors hover:border-accent/60 focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-accent/35"
                    aria-label={att.originalName}
                    title={att.originalName}
                  >
                    <img
                      src={`data:${att.mediaType};base64,${att.base64Data}`}
                      alt=""
                      className="h-full w-full object-cover"
                    />
                  </button>
                ) : (
                  <div className="flex h-10 w-10 items-center justify-center rounded-md border border-border bg-surface-2">
                    <FileText className="h-4 w-4 text-text-tertiary" />
                  </div>
                )}
                <button
                  type="button"
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

        <NexaPopoverAnchor asChild>
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
        </NexaPopoverAnchor>

        <div className="flex min-h-11 items-center justify-between gap-3 border-t border-border/35 px-2.5 py-2">
          <div className="flex min-w-0 flex-1 items-center gap-1.5 overflow-x-auto overflow-y-hidden">
            <NexaPopover
              open={workflowCatalogOpen && !slashMenuOpen}
              onOpenChange={setWorkflowCatalogOpen}
            >
            <NexaPopoverTrigger asChild>
            <button
              type="button"
              data-testid="workflow-catalog-trigger"
              disabled={attachmentLocked}
              className="flex h-8 shrink-0 items-center gap-1.5 rounded-md px-2 text-xs font-medium text-text-secondary transition-colors duration-fast ease-out hover:bg-surface-2 hover:text-text-primary disabled:pointer-events-none disabled:opacity-40"
              aria-label={t("chat.workflows")}
              aria-expanded={workflowCatalogOpen}
            >
              <Workflow className="h-3.5 w-3.5" />
              <span className="hidden sm:inline">{t("chat.workflows")}</span>
              <ChevronDown className={`h-3 w-3 transition-transform ${workflowCatalogOpen ? "rotate-180" : ""}`} />
            </button>
            </NexaPopoverTrigger>
            {workflowCatalogOpen && !slashMenuOpen && (
              <NexaPopoverContent
                data-testid="workflow-catalog-panel"
                side="top"
                align="start"
                collisionPadding={16}
                className="w-[min(64rem,calc(100vw-2rem))] overflow-hidden p-0"
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
              </NexaPopoverContent>
            )}
            </NexaPopover>

            {sessionControls}

            <label
              data-testid="chat-moa-control"
              className={`flex h-8 shrink-0 items-center gap-1 rounded-md border px-1.5 text-xs transition-colors ${
                moaModeEnabled
                  ? "border-cyan-400/35 bg-cyan-500/10 text-cyan-300"
                  : "border-transparent text-text-tertiary hover:border-border/70 hover:bg-surface-2"
              }`}
            >
              <Users className="h-3.5 w-3.5" />
              <NexaSelect
                data-testid="chat-moa-preset"
                aria-label={t("chat.moaMode")}
                value={moaModeEnabled ? moaPreset : "direct"}
                disabled={attachmentLocked}
                onChange={(event) => {
                  if (event.target.value === "direct") {
                    persistRuntimePolicy({ collaborationMode: "direct" });
                  } else {
                    persistRuntimePolicy({
                      collaborationMode: "mixtureOfAgents",
                      moaPreset: event.target.value as MoaPresetId,
                    });
                  }
                }}
                className="max-w-24 bg-transparent text-[11px] outline-none"
              >
                <option value="direct">{t("chat.moaOff")}</option>
                <option value="fastReview">{t("chat.moaPreset.fastReview")}</option>
                <option value="deepResearch">{t("chat.moaPreset.deepResearch")}</option>
                <option value="crossModelCodeReview">{t("chat.moaPreset.crossModelCodeReview")}</option>
                <option value="custom">{t("chat.moaPreset.custom")}</option>
              </NexaSelect>
            </label>

            <label
              data-testid="chat-quality-control"
              className={`flex h-8 shrink-0 items-center gap-1 rounded-md border px-1.5 text-xs transition-colors ${
                orchestrationProfile !== "balanced"
                  ? "border-amber-400/35 bg-amber-500/10 text-amber-300"
                  : "border-transparent text-text-tertiary hover:border-border/70 hover:bg-surface-2"
              }`}
            >
              <ShieldCheck className="h-3.5 w-3.5" />
              <NexaSelect
                data-testid="chat-quality-profile"
                aria-label={t("chat.qualityProfile")}
                value={orchestrationProfile}
                disabled={attachmentLocked}
                onChange={(event) => persistRuntimePolicy({
                  orchestrationProfile: event.target.value as OrchestrationProfile,
                })}
                className="max-w-24 bg-transparent text-[11px] outline-none"
              >
                <option value="balanced">{t("chat.qualityProfile.balanced")}</option>
                <option value="deep">{t("chat.qualityProfile.deep")}</option>
                <option value="codeUltra">{t("chat.qualityProfile.codeUltra")}</option>
                <option value="researchUltra">{t("chat.qualityProfile.researchUltra")}</option>
                <option value="custom">{t("chat.qualityProfile.custom")}</option>
              </NexaSelect>
            </label>

            <button
              type="button"
              data-testid="chat-nexus-mode"
              onClick={() => {
                if (nexusModeEnabled) {
                  setPowerMode("standard");
                } else if (hasAcknowledgedNexusMode()) {
                  activateNexusMode();
                } else {
                  setNexusDialogOpen(true);
                }
              }}
              disabled={attachmentLocked}
              aria-pressed={nexusModeEnabled}
              className={`flex h-8 shrink-0 items-center gap-1.5 rounded-md border px-2 text-xs font-medium transition-all duration-fast disabled:pointer-events-none disabled:opacity-40 ${
                nexusModeEnabled
                  ? "border-violet-400/35 bg-violet-500/12 text-violet-300 shadow-[0_0_16px_rgba(139,92,246,0.12)]"
                  : "border-transparent text-text-tertiary hover:border-border/70 hover:bg-surface-2 hover:text-text-primary"
              }`}
              aria-label={t("chat.nexusMode")}
            >
              <Sparkles className="h-3.5 w-3.5" />
              <span className="hidden sm:inline">Nexus</span>
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
                sendLocked ||
                (isStreaming
                  ? !value.trim() || attachments.length > 0
                  : !value.trim() && attachments.length === 0 && !activeSlashCommand)
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
      <AnimatePresence>
        {nexusActivationVisible && (
          <motion.div
            data-testid="nexus-activation-effect"
            aria-hidden="true"
            className="pointer-events-none fixed inset-0 z-[70] overflow-hidden"
            style={{
              background:
                "radial-gradient(ellipse at 50% 74%, rgba(139, 92, 246, 0.13) 0%, rgba(14, 7, 30, 0.62) 34%, rgba(2, 1, 8, 0.82) 100%)",
            }}
            initial={{ opacity: 0 }}
            animate={{ opacity: [0, 1, 0.92, 0] }}
            transition={{ duration: 1.35, times: [0, 0.12, 0.72, 1], ease: "easeOut" }}
            onAnimationComplete={() => setNexusActivationVisible(false)}
          >
            <motion.div
              className="absolute inset-x-0 top-[74%] h-px origin-center bg-linear-to-r from-transparent via-violet-100 to-transparent shadow-[0_0_34px_8px_rgba(139,92,246,0.72)]"
              initial={{ scaleX: 0.01, opacity: 0 }}
              animate={{ scaleX: [0.01, 1.15, 0.7], opacity: [0, 1, 0] }}
              transition={{ duration: 1.05, times: [0, 0.38, 1], ease: [0.16, 1, 0.3, 1] }}
            />
            {[0, 0.12, 0.24].map((delay, index) => (
              <div
                key={delay}
                className="absolute left-1/2 top-[74%] h-64 w-64 -translate-x-1/2 -translate-y-1/2"
              >
                <motion.div
                  className="h-full w-full rounded-full border border-violet-300/80"
                  style={{ boxShadow: "0 0 38px rgba(139, 92, 246, 0.28), inset 0 0 24px rgba(196, 181, 253, 0.12)" }}
                  initial={{ scale: 0.06, opacity: 0 }}
                  animate={{ scale: [0.06, 0.18, 2.9 + index * 0.42], opacity: [0, 0.88, 0] }}
                  transition={{ duration: 1.1, delay, times: [0, 0.18, 1], ease: [0.16, 1, 0.3, 1] }}
                />
              </div>
            ))}
            <div className="absolute left-1/2 top-[74%] -translate-x-1/2 -translate-y-1/2">
              <motion.div
                className="h-44 w-px origin-center bg-white shadow-[0_0_18px_5px_rgba(196,181,253,0.9),0_0_64px_18px_rgba(109,40,217,0.68)]"
                initial={{ scaleY: 0, opacity: 0 }}
                animate={{ scaleY: [0, 1, 0.06], opacity: [0, 1, 0] }}
                transition={{ duration: 0.82, times: [0, 0.32, 1], ease: [0.16, 1, 0.3, 1] }}
              />
            </div>
            <motion.div
              className="absolute inset-x-0 top-[54%] text-center font-mono text-base font-semibold tracking-[0.55em] text-violet-50 drop-shadow-[0_0_16px_rgba(196,181,253,1)]"
              initial={{ opacity: 0, y: 10, filter: "blur(8px)" }}
              animate={{ opacity: [0, 1, 0], y: [10, 0, -8], filter: ["blur(8px)", "blur(0px)", "blur(5px)"] }}
              transition={{ duration: 1.1, times: [0, 0.32, 1], ease: "easeOut" }}
            >
              NEXUS
            </motion.div>
          </motion.div>
        )}
      </AnimatePresence>
      <Modal
        open={previewAttachment !== null}
        onClose={() => setPreviewAttachment(null)}
        title={previewAttachment?.originalName ?? ""}
      >
        {previewAttachment && (
          <img
            data-testid="chat-attachment-preview"
            src={`data:${previewAttachment.mediaType};base64,${previewAttachment.base64Data}`}
            alt={previewAttachment.originalName}
            className="mx-auto max-h-[68vh] max-w-full rounded-lg object-contain"
          />
        )}
      </Modal>
      <Modal
        open={nexusDialogOpen}
        onClose={() => setNexusDialogOpen(false)}
        title={t("chat.nexusDialogTitle")}
        surfaceClassName="bg-surface-0"
        footer={(
          <>
            <button
              type="button"
              onClick={() => setNexusDialogOpen(false)}
              className="rounded-md px-3 py-2 text-sm text-text-secondary transition-colors hover:bg-surface-3 hover:text-text-primary"
            >
              {t("common.cancel")}
            </button>
            <button
              type="button"
              data-testid="chat-nexus-confirm"
              onClick={() => {
                if (nexusModeEnabled) {
                  setPowerMode("standard");
                  setNexusDialogOpen(false);
                } else {
                  activateNexusMode();
                }
              }}
              className={`rounded-md px-3 py-2 text-sm font-medium transition-colors ${
                nexusModeEnabled
                  ? "bg-danger/12 text-danger hover:bg-danger/20"
                  : "bg-violet-500 text-white hover:bg-violet-400"
              }`}
            >
              {t(nexusModeEnabled ? "chat.nexusDisable" : "chat.nexusEnable")}
            </button>
          </>
        )}
      >
        <div data-testid="chat-nexus-dialog" className="space-y-4">
          <div className="rounded-lg border border-violet-400/20 bg-violet-500/8 p-3">
            <div className="flex items-start gap-2.5">
              <TriangleAlert className="mt-0.5 h-4 w-4 shrink-0 text-violet-300" />
              <p className="text-sm leading-6 text-text-secondary">{t("chat.nexusDialogIntro")}</p>
            </div>
          </div>
          <div className="grid gap-2 sm:grid-cols-2">
            {nexusRiskItems.map(({ Icon, key }) => (
              <div key={key} className="flex gap-2.5 rounded-lg border border-border/70 bg-surface-1/70 p-3">
                <Icon className="mt-0.5 h-4 w-4 shrink-0 text-text-tertiary" />
                <p className="text-xs leading-5 text-text-secondary">{t(key)}</p>
              </div>
            ))}
          </div>
          <p className="text-xs leading-5 text-text-tertiary">{t("chat.nexusModelBound")}</p>
          <div className="grid gap-2 sm:grid-cols-2">
            <div className="rounded-lg border border-success/20 bg-success/8 p-3">
              <div className="mb-1 text-xs font-semibold text-success">{t("chat.nexusUseForTitle")}</div>
              <p className="text-xs leading-5 text-text-secondary">{t("chat.nexusUseFor")}</p>
            </div>
            <div className="rounded-lg border border-warning/20 bg-warning/8 p-3">
              <div className="mb-1 text-xs font-semibold text-warning">{t("chat.nexusAvoidForTitle")}</div>
              <p className="text-xs leading-5 text-text-secondary">{t("chat.nexusAvoidFor")}</p>
            </div>
          </div>
        </div>
      </Modal>
    </div>
    </NexaPopover>
  );
}
