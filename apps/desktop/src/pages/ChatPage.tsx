import { useCallback, useState, useEffect, useMemo, useRef, type CSSProperties, type KeyboardEvent as ReactKeyboardEvent, type ReactNode } from 'react';
import { createPortal } from 'react-dom';
import { useParams, useNavigate, useLocation } from 'react-router';
import { Archive, ArchiveRestore, Check, ChevronDown, Globe2, Loader2, Network, Settings, PanelLeftClose, PanelLeftOpen, TerminalSquare, UserRound, Volume2, VolumeX, X } from 'lucide-react';
import { AnimatePresence, motion } from 'framer-motion';
import { toast } from 'sonner';
import { Logo } from '../components/Logo';
import { SourceSelector, SystemPromptEditor, ChatSidebar, ChatInput, ActiveExtensions, ChatRunOverview, TaskBoard, AgentModelPicker, type AgentModelSelection, type ChatInputSendOptions } from '../components/chat';
import { ApprovalDialog } from '../components/chat/ApprovalDialog';
import {
  TerminalDock,
  TERMINAL_TOGGLE_EVENT,
  type TerminalAgentSelection,
} from '../components/chat/TerminalDock';
import { ChatMessages } from '../features/chat';
import { BrowserDock, type BrowserAgentArtifact, type BrowserDockStatus } from '../features/browser';
import { useApprovalQueue } from '../lib/useApprovalQueue';
import { useTranslation } from '../i18n';
import { EmptyState } from '../components/ui/EmptyState';
import { Button } from '../components/ui/Button';
import { useChatSession } from '../lib/useChatSession';
import { useResizablePanel } from '../lib/useResizablePanel';
import { undoableAction } from '../lib/undoToast';
import * as api from '../lib/api';
import type { AgentConfig, AppConfig, Conversation, ImageAttachment, SaveAgentConfigInput } from '../types/conversation';
import { formatUserError } from '../lib/userError';
import { useSpeechPlayback } from '../features/voice/SpeechPlaybackProvider';
import { isGoalMessage, isSteeringMessage } from '../lib/chatMessageGuards';
import { getActiveGoalContext } from '../lib/goalContext';
import {
  GRAPH_AGENT_CONTEXT_EVENT,
  buildGraphCollectionContext,
  clearGraphAgentContext,
  readGraphAgentContext,
  type GraphAgentContext,
} from '../lib/knowledgeGraphAgent';

function restoreConversationItems(
  current: Conversation[],
  previous: Conversation[],
  restoredIds: Set<string>,
): Conversation[] {
  const previousIds = new Set(previous.map((conversation) => conversation.id));
  const currentById = new Map(current.map((conversation) => [conversation.id, conversation]));
  const additions = current.filter((conversation) => !previousIds.has(conversation.id));
  const restored = previous.flatMap((conversation) => {
    const latest = currentById.get(conversation.id);
    if (latest) return [latest];
    return restoredIds.has(conversation.id) ? [conversation] : [];
  });
  return [...additions, ...restored];
}

function personaExists(personas: api.PersonaProfile[], id: string): boolean {
  return personas.some((persona) => persona.id === id && persona.enabled !== false);
}

function suggestPersonaId(message: string, personas: api.PersonaProfile[]): string | null {
  const text = message.toLowerCase();
  const matches = (terms: string[]) => terms.some((term) => text.includes(term.toLowerCase()));

  if (
    personaExists(personas, 'programmer') &&
    matches([
      'code',
      'coding',
      'program',
      'programmer',
      'developer',
      'debug',
      'bug',
      'stack trace',
      'typescript',
      'javascript',
      'rust',
      'python',
      'react',
      'repo',
      'repository',
      'refactor',
      'tests',
      'unit test',
      'integration test',
      'test failed',
      'lint',
      'build failed',
      '代码',
      '程序',
      '程序员',
      '开发',
      '调试',
      '报错',
      '修复',
      '重构',
      '测试',
      '仓库',
      '构建失败',
    ])
  ) {
    return 'programmer';
  }
  if (
    personaExists(personas, 'speaker') &&
    matches(['ppt', 'pptx', 'powerpoint', 'slide', 'slides', 'deck', 'presentation', 'pitch', '演讲', '讲稿', '幻灯', '汇报', '路演'])
  ) {
    return 'speaker';
  }
  if (
    personaExists(personas, 'researcher') &&
    matches(['research', 'investigate', 'citation', 'citations', 'evidence', 'source', 'sources', 'paper', '调研', '研究', '证据', '引用', '论文', '深入了解'])
  ) {
    return 'researcher';
  }
  if (
    personaExists(personas, 'editor') &&
    matches(['edit', 'rewrite', 'polish', 'proofread', 'copyedit', '润色', '改写', '校对', '编辑', '修改文案'])
  ) {
    return 'editor';
  }
  if (
    personaExists(personas, 'novelist') &&
    matches(['novel', 'fiction', 'story', 'scene', 'character', 'plot', '小说', '故事', '角色', '剧情', '场景'])
  ) {
    return 'novelist';
  }

  return null;
}

function buildApprovedPlanPrompt(planMarkdown: string): string {
  return [
    'Implement the approved plan below.',
    'Treat the plan as the source of truth for this turn. Make the required changes, run focused verification, and report exactly what changed and what was verified.',
    '',
    '<approved_plan>',
    planMarkdown.trim(),
    '</approved_plan>',
  ].join('\n');
}

function buildTerminalSelectionPrompt(selection: TerminalAgentSelection): string {
  const process = selection.session.processId ? ` · PID ${selection.session.processId}` : '';
  return [
    'Please diagnose the selected output from the Terminal linked to this chat.',
    `Terminal: ${selection.session.shell}${process}`,
    `Working directory: ${selection.session.cwd}`,
    'You may use the terminal_session tool to inspect newer output. Ask for approval before sending input to the live terminal.',
    '',
    '<terminal_selection>',
    selection.text.trim(),
    '</terminal_selection>',
  ].join('\n');
}

function buildBrowserArtifactPrompt(artifact: BrowserAgentArtifact): string {
  const summary = artifact.kind === 'element'
    ? `${artifact.role || artifact.tag} “${artifact.name || artifact.ref}”`
    : artifact.kind === 'region'
      ? `coordinate region ${Math.round(artifact.bounds.width)}×${Math.round(artifact.bounds.height)} (no pixel capture)`
      : `selected text (${artifact.text.length} characters)`;
  return [
    'Please use the Browser Workspace linked to this chat to work with the page context I selected.',
    `Page: ${artifact.title || artifact.url}`,
    `URL: ${artifact.url}`,
    `Selection: ${summary}`,
    'The artifact is observation-scoped. Re-observe the shared browser tab before acting if the page or control owner changed.',
    '',
    '<browser_artifact>',
    JSON.stringify(artifact, null, 2),
    '</browser_artifact>',
  ].join('\n');
}


function agentConfigToSaveInput(
  config: AgentConfig,
  patch: Pick<AgentModelSelection, 'model' | 'reasoningEnabled' | 'thinkingBudget' | 'reasoningEffort'>,
): SaveAgentConfigInput {
  return {
    id: config.id,
    name: config.name,
    provider: config.provider,
    apiKey: config.apiKey,
    baseUrl: config.baseUrl,
    model: patch.model,
    temperature: config.temperature,
    maxTokens: config.maxTokens,
    contextWindow: config.contextWindow,
    isDefault: true,
    reasoningEnabled: patch.reasoningEnabled,
    thinkingBudget: patch.thinkingBudget,
    reasoningEffort: patch.reasoningEffort,
    maxIterations: config.maxIterations,
    summarizationModel: config.summarizationModel,
    summarizationProvider: config.summarizationProvider,
    imageGenerationModel: config.imageGenerationModel,
    subagentAllowedTools: config.subagentAllowedTools,
    subagentAllowedSkillIds: config.subagentAllowedSkillIds,
    subagentMaxParallel: config.subagentMaxParallel,
    subagentMaxCallsPerTurn: config.subagentMaxCallsPerTurn,
    subagentTokenBudget: config.subagentTokenBudget,
    dynamicToolVisibility: config.dynamicToolVisibility,
    traceEnabled: config.traceEnabled,
    requireToolConfirmation: config.requireToolConfirmation,
  };
}

const CHAT_SIDEBAR_WIDTH_KEY = 'chat-sidebar-width';
const CHAT_SIDEBAR_MIN_WIDTH = 200;
const CHAT_SIDEBAR_MAX_WIDTH = 420;

interface SessionSelectProps {
  icon: ReactNode;
  label: string;
  value: string;
  detail?: string;
  selectValue: string;
  title: string;
  ariaLabel: string;
  tone?: 'accent' | 'info';
  onChange: (value: string) => void | Promise<void>;
  options: SessionSelectOption[];
}

interface SessionSelectOption {
  value: string;
  label: string;
  detail?: string;
  icon?: ReactNode;
}

function SessionSelect({
  icon,
  label,
  value,
  detail,
  selectValue,
  title,
  ariaLabel,
  tone = 'accent',
  onChange,
  options,
}: SessionSelectProps) {
  const [open, setOpen] = useState(false);
  const [panelStyle, setPanelStyle] = useState<CSSProperties>({});
  const ref = useRef<HTMLDivElement>(null);
  const triggerRef = useRef<HTMLButtonElement>(null);
  const panelRef = useRef<HTMLDivElement>(null);
  const iconClass = tone === 'info' ? 'text-info' : 'text-accent';
  const controlTitle = detail ? `${title} · ${detail}` : title;
  const selectedOption = options.find((option) => option.value === selectValue);
  const toneClasses = tone === 'info'
    ? {
        icon: 'border-info/25 bg-info/10 text-info',
        active: 'bg-info/10 text-text-primary ring-1 ring-info/25',
        activeIcon: 'border-info/35 bg-info/10 text-info',
        check: 'text-info',
      }
    : {
        icon: 'border-accent/25 bg-accent/10 text-accent',
        active: 'bg-accent-subtle text-text-primary ring-1 ring-accent/25',
        activeIcon: 'border-accent/35 bg-accent/10 text-accent',
        check: 'text-accent',
      };

  const updatePanelPosition = useCallback(() => {
    const rect = triggerRef.current?.getBoundingClientRect();
    if (!rect) return;
    const width = Math.min(320, Math.max(240, window.innerWidth - 16));
    const left = Math.max(8, Math.min(rect.left, window.innerWidth - width - 8));
    setPanelStyle({
      bottom: window.innerHeight - rect.top + 8,
      left,
      width,
    });
  }, []);

  const closeMenu = useCallback(() => setOpen(false), []);

  useEffect(() => {
    if (!open) return;
    updatePanelPosition();
    const handlePointerDown = (event: MouseEvent) => {
      if (ref.current?.contains(event.target as Node)) return;
      if (panelRef.current?.contains(event.target as Node)) return;
      closeMenu();
    };
    const handleKeyDown = (event: KeyboardEvent) => {
      if (event.key === 'Escape') {
        event.preventDefault();
        closeMenu();
        triggerRef.current?.focus();
      }
    };
    window.addEventListener('resize', updatePanelPosition);
    window.addEventListener('scroll', updatePanelPosition, true);
    document.addEventListener('mousedown', handlePointerDown);
    document.addEventListener('keydown', handleKeyDown);
    return () => {
      window.removeEventListener('resize', updatePanelPosition);
      window.removeEventListener('scroll', updatePanelPosition, true);
      document.removeEventListener('mousedown', handlePointerDown);
      document.removeEventListener('keydown', handleKeyDown);
    };
  }, [closeMenu, open, updatePanelPosition]);

  const handleSelect = useCallback((nextValue: string) => {
    setOpen(false);
    if (nextValue !== selectValue) {
      void onChange(nextValue);
    }
    requestAnimationFrame(() => triggerRef.current?.focus());
  }, [onChange, selectValue]);

  return (
    <div ref={ref} className="relative shrink-0">
      <button
        ref={triggerRef}
        type="button"
        className={`group flex h-8 w-8 shrink-0 cursor-pointer items-center justify-center gap-0
          overflow-hidden rounded-md px-2 text-xs font-medium transition-colors duration-fast ease-out
          hover:bg-surface-2 focus-visible:bg-surface-2 focus-visible:outline-none focus-visible:ring-2
          focus-visible:ring-accent/20 sm:w-auto sm:max-w-[10rem] sm:justify-start sm:gap-1.5 ${
            open ? 'bg-surface-2 text-text-primary' : 'text-text-secondary hover:text-text-primary'
          }`}
        aria-haspopup="listbox"
        aria-expanded={open}
        aria-label={ariaLabel}
        title={controlTitle}
        onClick={() => setOpen((current) => !current)}
        onKeyDown={(event) => {
          if (event.key === 'ArrowDown' || event.key === 'Enter' || event.key === ' ') {
            event.preventDefault();
            setOpen(true);
          }
        }}
      >
        <span className={`flex h-4 w-4 shrink-0 items-center justify-center ${iconClass}`}>
          {icon}
        </span>
        <span className="hidden min-w-0 sm:flex">
          <span className="truncate text-xs font-medium text-text-secondary group-hover:text-text-primary">
            {value}
          </span>
        </span>
        <ChevronDown className={`hidden h-3 w-3 shrink-0 text-text-tertiary transition-transform group-hover:text-text-secondary sm:block ${open ? 'rotate-180' : ''}`} />
      </button>

      {createPortal(
        <AnimatePresence>
          {open && (
            <motion.div
              ref={panelRef}
              initial={{ opacity: 0, y: 4, scale: 0.98 }}
              animate={{ opacity: 1, y: 0, scale: 1 }}
              exit={{ opacity: 0, y: 4, scale: 0.98 }}
              transition={{ duration: 0.14, ease: [0.16, 1, 0.3, 1] }}
              className="fixed z-50 overflow-hidden rounded-lg border border-border/70 bg-surface-0
                shadow-2xl shadow-black/25 ring-1 ring-white/[0.04]"
              style={panelStyle}
              role="listbox"
              aria-label={ariaLabel}
            >
              <div className="flex min-w-0 items-center gap-2 border-b border-border/60 px-3 py-2">
                <span className={`flex h-7 w-7 shrink-0 items-center justify-center rounded-md border ${toneClasses.icon}`}>
                  {icon}
                </span>
                <div className="min-w-0">
                  <div className="text-xs font-medium text-text-primary">{label}</div>
                  <div className="truncate text-[11px] text-text-tertiary">
                    {selectedOption?.label ?? value}
                  </div>
                </div>
              </div>
              <div className="max-h-72 overflow-y-auto p-1">
                {options.map((option) => {
                  const selected = option.value === selectValue;
                  return (
                    <button
                      key={option.value}
                      type="button"
                      role="option"
                      aria-selected={selected}
                      onClick={() => handleSelect(option.value)}
                      className={`grid w-full grid-cols-[1.75rem_minmax(0,1fr)_1rem] items-center gap-2 rounded-md px-2 py-2 text-left transition-colors ${
                        selected
                          ? toneClasses.active
                          : 'text-text-secondary hover:bg-surface-1 hover:text-text-primary'
                      }`}
                    >
                      <span
                        className={`flex h-7 w-7 items-center justify-center rounded-md border ${
                          selected
                            ? toneClasses.activeIcon
                            : 'border-border/60 bg-surface-1 text-text-tertiary'
                        }`}
                      >
                        {option.icon ?? icon}
                      </span>
                      <span className="min-w-0">
                        <span className="block truncate text-sm font-medium text-text-primary">
                          {option.label}
                        </span>
                        {option.detail && (
                          <span className="mt-0.5 block truncate text-[11px] leading-4 text-text-tertiary">
                            {option.detail}
                          </span>
                        )}
                      </span>
                      {selected && <Check className={`h-3.5 w-3.5 ${toneClasses.check}`} />}
                    </button>
                  );
                })}
              </div>
            </motion.div>
          )}
        </AnimatePresence>,
        document.body,
      )}
    </div>
  );
}

/* ------------------------------------------------------------------ */
/*  Component                                                          */
/* ------------------------------------------------------------------ */

export function ChatPage() {
  const { t } = useTranslation();
  const { conversationId } = useParams<{ conversationId?: string }>();
  const navigate = useNavigate();
  const location = useLocation();

  const onConversationCreated = useCallback(
    (id: string) => navigate(`/chat/${id}`, { replace: true }),
    [navigate],
  );

  const initialSourceIds = (
    (location.state as { sourceIds?: string[] } | null)?.sourceIds ?? []
  ).filter((value): value is string => typeof value === 'string' && value.length > 0);
  const initialCollectionContext = (
    (location.state as { collectionContext?: Conversation['collectionContext'] } | null)?.collectionContext
  ) ?? null;

  // Source scope forwarded from route state, applied when the first send
  // auto-creates a conversation.
  const currentSourceIdsRef = useRef<string[]>(initialSourceIds);
  useEffect(() => {
    currentSourceIdsRef.current = initialSourceIds;
  }, [initialSourceIds]);
  const getCurrentSourceScope = useCallback(
    () => currentSourceIdsRef.current,
    [],
  );
  const handleSourceSelectionChange = useCallback((ids: string[]) => {
    currentSourceIdsRef.current = ids;
  }, []);

  const [activePersonaId, setActivePersonaId] = useState('default');
  const [personas, setPersonas] = useState<api.PersonaProfile[]>([]);
  useEffect(() => {
    api.listPersonas()
      .then((items) => setPersonas(Array.isArray(items) ? items : []))
      .catch(() => setPersonas([]));
  }, []);
  const chat = useChatSession({
    conversationId,
    onConversationCreated,
    systemPrompt: ((location.state as { systemPrompt?: string } | null)?.systemPrompt ?? '').trim(),
    initialSourceIds,
    getCurrentSourceScope,
    initialCollectionContext,
    activePersonaId,
  });
  const [appConfig, setAppConfig] = useState<AppConfig | null>(null);
  const [autoSpeechSaving, setAutoSpeechSaving] = useState(false);
  const { speakMessage, stop: stopSpeech } = useSpeechPlayback();
  const finalSpeechCandidateRef = useRef<{ conversationId: string; text: string } | null>(null);
  const autoSpeechEnabled = appConfig?.textToSpeech?.autoSpeakFinalAnswers === true;

  useEffect(() => {
    let cancelled = false;
    api.getAppConfig()
      .then((config) => {
        if (!cancelled) setAppConfig(config);
      })
      .catch(() => undefined);
    return () => {
      cancelled = true;
      stopSpeech();
    };
  }, [stopSpeech]);

  useEffect(() => {
    if (!autoSpeechEnabled || !chat.activeId || !chat.isStreaming || !chat.streamText.trim()) return;
    finalSpeechCandidateRef.current = {
      conversationId: chat.activeId,
      text: chat.streamText,
    };
  }, [autoSpeechEnabled, chat.activeId, chat.isStreaming, chat.streamText]);

  useEffect(() => {
    if (chat.isStreaming) return;
    const candidate = finalSpeechCandidateRef.current;
    if (!candidate || candidate.conversationId !== chat.activeId) return;
    finalSpeechCandidateRef.current = null;
    if (!autoSpeechEnabled || chat.activeConversation?.archivedAt) return;
    void speakMessage(`auto:${candidate.conversationId}`, candidate.text);
  }, [autoSpeechEnabled, chat.activeConversation, chat.activeId, chat.isStreaming, speakMessage]);

  const toggleAutoSpeech = useCallback(async () => {
    if (!appConfig?.textToSpeech || autoSpeechSaving) return;
    const nextEnabled = !appConfig.textToSpeech.autoSpeakFinalAnswers;
    if (nextEnabled) {
      const tts = appConfig.textToSpeech;
      const configured = tts.apiStyle === 'sherpa_onnx'
        ? Boolean(tts.executablePath?.trim() && tts.modelPath?.trim() && tts.tokensPath?.trim())
        : Boolean(tts.apiKey.trim() && tts.model.trim() && tts.voice.trim());
      if (!configured) {
        toast.error(t('chat.autoTtsNeedsProvider'));
        return;
      }
    } else {
      stopSpeech();
      finalSpeechCandidateRef.current = null;
    }
    const nextConfig: AppConfig = {
      ...appConfig,
      textToSpeech: {
        ...appConfig.textToSpeech,
        autoSpeakFinalAnswers: nextEnabled,
      },
    };
    setAppConfig(nextConfig);
    setAutoSpeechSaving(true);
    try {
      await api.saveAppConfig(nextConfig);
    } catch (error) {
      setAppConfig(appConfig);
      toast.error(formatUserError(t('chat.autoTtsFailed'), error));
    } finally {
      setAutoSpeechSaving(false);
    }
  }, [appConfig, autoSpeechSaving, stopSpeech, t]);
  const chatInputHistory = useMemo(
    () => chat.messages
      .filter((message) => (
        message.role === 'user' &&
        !isSteeringMessage(message) &&
        !isGoalMessage(message) &&
        message.content.trim().length > 0
      ))
      .map((message) => message.content),
    [chat.messages],
  );
  const activeGoalContext = useMemo(
    () => getActiveGoalContext(chat.messages, chat.toolCalls),
    [chat.messages, chat.toolCalls],
  );
  const [pendingGraphContext, setPendingGraphContext] = useState<GraphAgentContext | null>(
    () => readGraphAgentContext(),
  );
  const [planModeEnabled, setPlanModeEnabled] = useState(false);

  useEffect(() => {
    setPlanModeEnabled(false);
  }, [chat.activeId]);

  useEffect(() => {
    const syncGraphContext = () => setPendingGraphContext(readGraphAgentContext());
    window.addEventListener(GRAPH_AGENT_CONTEXT_EVENT, syncGraphContext as EventListener);
    window.addEventListener('storage', syncGraphContext);
    return () => {
      window.removeEventListener(GRAPH_AGENT_CONTEXT_EVENT, syncGraphContext as EventListener);
      window.removeEventListener('storage', syncGraphContext);
    };
  }, []);

  useEffect(() => {
    if (!chat.activeId) return;
    const next = chat.activeConversation?.personaId || 'default';
    setActivePersonaId((current) => (current === next ? current : next));
  }, [chat.activeId, chat.activeConversation?.personaId]);

  const setPersona = useCallback((id: string) => {
    setActivePersonaId(id);
    if (chat.activeId) {
      void api.updateConversationPersona(chat.activeId, id)
        .then((updated) => {
          chat.setConversations((prev) =>
            prev.map((conv) => (conv.id === updated.id ? updated : conv)),
          );
        })
        .catch((error) => toast.error(formatUserError(t('settings.personas'), error)));
    }
  }, [chat.activeId, chat.setConversations, t]);

  const handleChatSend = useCallback(
    async (content: string, attachments?: ImageAttachment[], inputOptions?: ChatInputSendOptions) => {
      const suggestedPersonaId =
        activePersonaId === 'default' ? suggestPersonaId(content, personas) : null;
      const personaForSend = suggestedPersonaId ?? activePersonaId;
      if (suggestedPersonaId && suggestedPersonaId !== activePersonaId) {
        setPersona(suggestedPersonaId);
      }
      const graphContext = pendingGraphContext;
      if (graphContext?.sourceId) {
        currentSourceIdsRef.current = [graphContext.sourceId];
      }
      const contextContent = inputOptions?.userArtifacts && !Array.isArray(inputOptions.userArtifacts)
        ? inputOptions.userArtifacts.llmContextContent
        : null;
      const userArtifacts =
        graphContext && inputOptions?.userArtifacts
          ? {
              kind: 'chatSendContext',
              graphContext,
              slashCommand: inputOptions.userArtifacts,
              ...(typeof contextContent === 'string' ? { llmContextContent: contextContent } : {}),
            }
          : graphContext
            ? {
                kind: 'graphAgentContext',
                graphContext,
              }
            : inputOptions?.userArtifacts;
      await chat.send(
        content,
        attachments,
        personaForSend,
        graphContext || inputOptions
          ? {
              ...(graphContext
                ? {
                    collectionContext: buildGraphCollectionContext(graphContext),
                    sourceIds: graphContext.sourceId ? [graphContext.sourceId] : [],
                  }
                : {}),
              skillIds: inputOptions?.skillIds,
              userArtifacts: userArtifacts ?? null,
              executionMode: inputOptions?.executionMode,
              powerMode: inputOptions?.powerMode,
              taskOrchestratorRunId: inputOptions?.taskOrchestratorRunId,
            }
          : undefined,
      );
      if (graphContext) {
        clearGraphAgentContext();
        setPendingGraphContext(null);
      }
    },
    [activePersonaId, chat.send, pendingGraphContext, personas, setPersona],
  );

  const handleApprovePlan = useCallback(
    (planMarkdown: string, sourceMessageId: string) => {
      setPlanModeEnabled(false);
      const prompt = buildApprovedPlanPrompt(planMarkdown);
      void handleChatSend(prompt, undefined, {
        executionMode: 'normal',
        userArtifacts: {
          kind: 'approvedPlan',
          version: 1,
          sourceMessageId,
          plan: planMarkdown,
        },
      });
    },
    [handleChatSend],
  );

  const handleClearGraphContext = useCallback(() => {
    clearGraphAgentContext();
    setPendingGraphContext(null);
  }, []);

  const handleToggleTerminal = useCallback(() => {
    window.dispatchEvent(new Event(TERMINAL_TOGGLE_EVENT));
  }, []);
  const [browserOpen, setBrowserOpen] = useState(false);
  const [browserStatus, setBrowserStatus] = useState<BrowserDockStatus>({ tabCount: 0, state: 'empty' });
  const handleToggleBrowser = useCallback(() => setBrowserOpen((value) => !value), []);
  useEffect(() => {
    const handler = (event: KeyboardEvent) => {
      if ((event.ctrlKey || event.metaKey) && event.shiftKey && event.key.toLowerCase() === 'b') {
        event.preventDefault();
        handleToggleBrowser();
      }
    };
    window.addEventListener('keydown', handler);
    return () => window.removeEventListener('keydown', handler);
  }, [handleToggleBrowser]);

  const [agentConfigs, setAgentConfigs] = useState<AgentConfig[]>([]);
  useEffect(() => {
    api.listAgentConfigs().then(setAgentConfigs);
  }, []);
  const selectedAgentConfig =
    agentConfigs.find((config) => config.id === chat.agentConfig?.id) ?? chat.agentConfig;
  const selectedPersona = personas.find((persona) => persona.id === activePersonaId);
  const selectedPersonaLabel = selectedPersona?.name || activePersonaId;
  const selectedPersonaDetail = selectedPersona?.description || activePersonaId;
  const handleAgentModelSelection = useCallback(
    async (selection: AgentModelSelection) => {
      const config = selection.config;
      const unchanged =
        config.model === selection.model &&
        config.reasoningEnabled === selection.reasoningEnabled &&
        config.thinkingBudget === selection.thinkingBudget &&
        config.reasoningEffort === selection.reasoningEffort;

      try {
        let nextConfig = config;
        if (!unchanged) {
          nextConfig = await api.saveAgentConfig(agentConfigToSaveInput(config, selection));
        }

        await chat.switchAgentConfig(nextConfig);
        setAgentConfigs((current) =>
          current.map((candidate) =>
            candidate.id === nextConfig.id
              ? { ...nextConfig, isDefault: true }
              : { ...candidate, isDefault: false },
          ),
        );
      } catch (error) {
        toast.error(formatUserError(t('settings.defaultModel'), error));
      }
    },
    [chat, t],
  );
  const sessionControls = (chat.agentConfig && agentConfigs.length > 0) || personas.length > 0 ? (
    <div className="flex shrink-0 items-center gap-1.5">
      {chat.agentConfig && agentConfigs.length > 0 && selectedAgentConfig && (
        <AgentModelPicker
          agentConfigs={agentConfigs}
          selectedConfig={selectedAgentConfig}
          onSelect={handleAgentModelSelection}
        />
      )}
      {personas.length > 0 && (
        <SessionSelect
          icon={<UserRound className="h-3.5 w-3.5" />}
          label={t('settings.personas')}
          value={selectedPersonaLabel}
          detail={selectedPersonaDetail}
          selectValue={activePersonaId}
          ariaLabel={t('settings.personas')}
          title={selectedPersonaLabel}
          tone="info"
          onChange={setPersona}
          options={personas.map((persona) => ({
            value: persona.id,
            label: persona.name,
            detail: persona.description,
            icon: <UserRound className="h-3.5 w-3.5" />,
          }))}
        />
      )}
    </div>
  ) : undefined;
  const collectionContext = chat.activeConversation?.collectionContext ?? initialCollectionContext;

  const sentInitialRef = useRef<string | null>(null);
  const initialMessage = (
    (location.state as { initialMessage?: string } | null)?.initialMessage ?? ''
  ).trim();
  const initialSystemPrompt = (
    (location.state as { systemPrompt?: string } | null)?.systemPrompt ?? ''
  ).trim();
  const initialTaskOrchestratorRunId = (
    (location.state as { taskOrchestratorRunId?: string | null } | null)?.taskOrchestratorRunId ?? ''
  ).trim();
  const initialSourceScopeKey = initialSourceIds.join(',');
  const initialCollectionKey = collectionContext ? JSON.stringify(collectionContext) : '';

  // Accept one-off initial message forwarded from other pages.
  useEffect(() => {
    if (!initialMessage || chat.loadingConfig || !chat.agentConfig || chat.isStreaming) {
      return;
    }
    const key = `${location.key}:${initialMessage}:${initialSystemPrompt}:${initialSourceScopeKey}:${initialCollectionKey}:${initialTaskOrchestratorRunId}`;
    if (sentInitialRef.current === key) {
      return;
    }
    sentInitialRef.current = key;
    void (async () => {
      if (conversationId && initialSourceIds.length > 0) {
        await api.setConversationSources(conversationId, initialSourceIds).catch(() => undefined);
      }
      if (conversationId && initialCollectionContext) {
        await api.updateConversationCollectionContext(conversationId, initialCollectionContext).catch(() => undefined);
      }
      await chat.send(
        initialMessage,
        undefined,
        undefined,
        initialTaskOrchestratorRunId
          ? { taskOrchestratorRunId: initialTaskOrchestratorRunId }
          : undefined,
      );
    })();

    const cleanPath = conversationId ? `/chat/${conversationId}` : '/chat';
    navigate(cleanPath, { replace: true, state: null });
  }, [
    initialMessage,
    initialSystemPrompt,
    initialTaskOrchestratorRunId,
    initialCollectionContext,
    initialCollectionKey,
    initialSourceIds,
    initialSourceScopeKey,
    chat.loadingConfig,
    chat.agentConfig,
    chat.isStreaming,
    chat.send,
    location.key,
    conversationId,
    navigate,
  ]);

  /* ── Sidebar collapsed state ──────────────────────────────────────── */

  const SIDEBAR_STORAGE_KEY = 'chat-sidebar-collapsed';
  const [sidebarCollapsed, setSidebarCollapsed] = useState(() => {
    try { return localStorage.getItem(SIDEBAR_STORAGE_KEY) === 'true'; } catch { return false; }
  });
  const {
    size: chatSidebarWidth,
    setSize: setChatSidebarWidth,
    startResize: startChatSidebarResize,
    isResizing: isChatSidebarResizing,
  } = useResizablePanel({
    storageKey: CHAT_SIDEBAR_WIDTH_KEY,
    defaultSize: 240,
    minSize: CHAT_SIDEBAR_MIN_WIDTH,
    maxSize: CHAT_SIDEBAR_MAX_WIDTH,
  });

  const toggleSidebar = useCallback(() => {
    setSidebarCollapsed((prev) => {
      const next = !prev;
      try { localStorage.setItem(SIDEBAR_STORAGE_KEY, String(next)); } catch { /* ignore */ }
      return next;
    });
  }, []);

  const handleChatSidebarResizeKey = useCallback((event: ReactKeyboardEvent<HTMLDivElement>) => {
    if (event.key === 'ArrowLeft') {
      event.preventDefault();
      setChatSidebarWidth(chatSidebarWidth - 12);
    } else if (event.key === 'ArrowRight') {
      event.preventDefault();
      setChatSidebarWidth(chatSidebarWidth + 12);
    } else if (event.key === 'Home') {
      event.preventDefault();
      setChatSidebarWidth(CHAT_SIDEBAR_MIN_WIDTH);
    } else if (event.key === 'End') {
      event.preventDefault();
      setChatSidebarWidth(CHAT_SIDEBAR_MAX_WIDTH);
    }
  }, [chatSidebarWidth, setChatSidebarWidth]);

  // Auto-collapse on narrow viewports
  useEffect(() => {
    const mq = window.matchMedia('(max-width: 767px)');
    const handler = (e: MediaQueryListEvent | MediaQueryList) => {
      if (e.matches) setSidebarCollapsed(true);
    };
    handler(mq);
    mq.addEventListener('change', handler);
    return () => mq.removeEventListener('change', handler);
  }, []);

  // Ctrl+B to toggle sidebar
  useEffect(() => {
    const handler = (e: KeyboardEvent) => {
      if ((e.ctrlKey || e.metaKey) && !e.shiftKey && e.key.toLowerCase() === 'b') {
        e.preventDefault();
        toggleSidebar();
      }
    };
    window.addEventListener('keydown', handler);
    return () => window.removeEventListener('keydown', handler);
  }, [toggleSidebar]);

  /* ── Handlers (navigation-aware wrappers) ───────────────────────── */

  const handleSelectConversation = useCallback(
    (id: string) => navigate(`/chat/${id}`),
    [navigate],
  );

  const handleNewConversation = useCallback(async (projectId?: string | null) => {
    // Defensive guard: if a React SyntheticEvent / DOM node leaks in as projectId
    // (e.g. onClick={handler} passes MouseEvent), drop it to avoid circular JSON.
    if (projectId != null && typeof projectId !== 'string') {
      if (import.meta.env.DEV) {
        // eslint-disable-next-line no-console
        console.warn('[ChatPage] handleNewConversation received non-string projectId; ignoring.', projectId);
      }
      projectId = null;
    }
    if (!chat.agentConfig) {
      navigate('/chat');
      chat.createNewConversation();
      return;
    }

    try {
      // If creating within a project, fetch project's system prompt as default
      let systemPrompt = chat.customSystemPrompt || undefined;
      if (projectId && !chat.customSystemPrompt) {
        try {
          const proj = await api.getProject(projectId);
          if (proj.systemPrompt) systemPrompt = proj.systemPrompt;
        } catch { /* ignore, use default */ }
      }

      const conv = await api.createConversation(
        chat.agentConfig.provider,
        chat.agentConfig.model,
        systemPrompt,
        projectId ?? undefined,
        'default',
      );
      chat.setConversations((prev) => [conv, ...prev.filter((c) => c.id !== conv.id)]);
      navigate(`/chat/${conv.id}`);
    } catch (e) {
      toast.error(formatUserError(t('chat.createError'), e));
      navigate('/chat');
      chat.createNewConversation();
    }
  }, [
    chat.agentConfig,
    chat.customSystemPrompt,
    chat.setConversations,
    chat.createNewConversation,
    navigate,
    t,
  ]);

  const handleCheckpointBranch = useCallback((conversation: Conversation) => {
    chat.setConversations((prev) => [conversation, ...prev.filter((c) => c.id !== conversation.id)]);
    navigate(`/chat/${conversation.id}`);
  }, [chat.setConversations, navigate]);

  const handleDeleteConversation = useCallback(
    (id: string) => {
      if (chat.runningConversationIds.has(id)) {
        toast.error(t('chat.stopBeforeArchive'));
        return;
      }
      const prev = chat.conversations;
      const removed = prev.find((c) => c.id === id);
      const wasActive = chat.activeId === id;
      chat.setConversations(prev.filter((c) => c.id !== id));
      if (wasActive) navigate('/chat');
      const restore = () => {
        chat.setConversations((current) => restoreConversationItems(current, prev, new Set([id])));
        if (wasActive) navigate(`/chat/${id}`);
      };
      undoableAction({
        message: t('chat.conversation.deleted'),
        undoLabel: t('common.undo'),
        onUndo: restore,
        onConfirm: async () => {
          try {
            await api.deleteConversation(id);
          } catch (e) {
            toast.error(formatUserError(t('chat.deleteError'), e));
            if (removed) restore();
          }
        },
      });
    },
    [chat.conversations, chat.runningConversationIds, chat.setConversations, chat.activeId, navigate, t],
  );

  const handleArchiveConversation = useCallback(
    async (id: string) => {
      if (chat.runningConversationIds.has(id)) {
        toast.error(t('chat.stopBeforeArchive'));
        return;
      }
      const prev = chat.conversations;
      const archived = prev.find((c) => c.id === id);
      if (!archived) return;
      const wasActive = chat.activeId === id;
      chat.setConversations(prev.filter((c) => c.id !== id));
      if (wasActive) navigate('/chat');
      const restoreArchived = (conversation: Conversation = archived) => {
        chat.setConversations((current) => [
          conversation,
          ...current.filter((item) => item.id !== id),
        ]);
        if (wasActive) navigate(`/chat/${id}`);
      };
      try {
        await api.archiveConversation(id);
        undoableAction({
          message: t('chat.conversation.archived'),
          undoLabel: t('common.undo'),
          onUndo: async () => {
            try {
              const restored = await api.unarchiveConversation(id);
              restoreArchived(restored);
            } catch (e) {
              toast.error(formatUserError(t('chat.unarchiveError'), e));
            }
          },
          onConfirm: () => {},
        });
      } catch (e) {
        toast.error(formatUserError(t('chat.archiveError'), e));
        chat.setConversations((current) => restoreConversationItems(current, prev, new Set([id])));
        if (wasActive) navigate(`/chat/${id}`);
      }
    },
    [chat.conversations, chat.runningConversationIds, chat.setConversations, chat.activeId, navigate, t],
  );

  const handleSelectArchivedConversation = useCallback((id: string) => {
    navigate(`/chat/${id}`);
  }, [navigate]);

  const handleArchivedConversationRestored = useCallback((conversation: Conversation) => {
    chat.setConversations((current) => [
      conversation,
      ...current.filter((item) => item.id !== conversation.id),
    ]);
  }, [chat.setConversations]);

  const handleArchivedConversationDeleted = useCallback((id: string) => {
    if (chat.activeId === id) {
      navigate('/chat');
    }
  }, [chat.activeId, navigate]);

  const [restoringArchivedConversation, setRestoringArchivedConversation] = useState(false);
  const handleRestoreActiveArchivedConversation = useCallback(async () => {
    const conversation = chat.activeConversation;
    if (!conversation?.archivedAt || restoringArchivedConversation) return;

    setRestoringArchivedConversation(true);
    try {
      const restored = await api.unarchiveConversation(conversation.id);
      handleArchivedConversationRestored(restored);
      await chat.loadConversations().catch(() => undefined);
      toast.success(t('chat.conversation.unarchived'));
    } catch (error) {
      toast.error(formatUserError(t('chat.unarchiveError'), error));
    } finally {
      setRestoringArchivedConversation(false);
    }
  }, [
    chat.activeConversation,
    chat.loadConversations,
    handleArchivedConversationRestored,
    restoringArchivedConversation,
    t,
  ]);

  const isArchivedConversation = Boolean(chat.activeConversation?.archivedAt);

  const handleDeleteBatch = useCallback(
    (ids: string[]) => {
      if (ids.some((id) => chat.runningConversationIds.has(id))) {
        toast.error(t('chat.stopBeforeArchive'));
        return;
      }
      const prev = chat.conversations;
      const idSet = new Set(ids);
      const removed = prev.filter((c) => idSet.has(c.id));
      const previousActiveId = chat.activeId && idSet.has(chat.activeId) ? chat.activeId : null;
      chat.setConversations(prev.filter((c) => !idSet.has(c.id)));
      if (previousActiveId) navigate('/chat');
      const restore = () => {
        chat.setConversations((current) => restoreConversationItems(current, prev, idSet));
        if (previousActiveId) navigate(`/chat/${previousActiveId}`);
      };
      undoableAction({
        message: t('chat.conversation.deleted'),
        undoLabel: t('common.undo'),
        onUndo: restore,
        onConfirm: async () => {
          try {
            await api.deleteConversationsBatch(ids);
          } catch (e) {
            toast.error(formatUserError(t('chat.deleteError'), e));
            if (removed.length > 0) restore();
          }
        },
      });
    },
    [chat.conversations, chat.runningConversationIds, chat.setConversations, chat.activeId, navigate, t],
  );

  const handleDeleteAll = useCallback(() => {
    if (chat.runningConversationIds.size > 0) {
      toast.error(t('chat.stopBeforeArchive'));
      return;
    }
    const prev = chat.conversations;
    const previousActiveId = chat.activeId;
    chat.setConversations([]);
    navigate('/chat');
    const restore = () => {
      chat.setConversations((current) =>
        restoreConversationItems(current, prev, new Set(prev.map((conversation) => conversation.id))),
      );
      if (previousActiveId) navigate(`/chat/${previousActiveId}`);
    };
    undoableAction({
      message: t('chat.conversation.deleted'),
      undoLabel: t('common.undo'),
      onUndo: restore,
      onConfirm: async () => {
        try {
          await api.deleteAllConversations();
        } catch (e) {
          toast.error(formatUserError(t('chat.deleteError'), e));
          restore();
        }
      },
    });
  }, [chat.conversations, chat.runningConversationIds, chat.setConversations, chat.activeId, navigate, t]);

  /* ── Suggestion prefill ─────────────────────────────────────────── */

  const [prefillText, setPrefillText] = useState<string>('');
  const [prefillKey, setPrefillKey] = useState(0);
  const handleSuggestionClick = useCallback((text: string) => {
    setPrefillText(text);
    setPrefillKey((current) => current + 1);
  }, []);
  const handleTerminalSelection = useCallback((selection: TerminalAgentSelection) => {
    handleSuggestionClick(buildTerminalSelectionPrompt(selection));
  }, [handleSuggestionClick]);
  const handleBrowserArtifact = useCallback((artifact: BrowserAgentArtifact) => {
    handleSuggestionClick(buildBrowserArtifactPrompt(artifact));
  }, [handleSuggestionClick]);

  const [isCompacting, setIsCompacting] = useState(false);
  const [compactCompleteVisible, setCompactCompleteVisible] = useState(false);
  const handleCompactConversation = useCallback(async () => {
    if (!chat.activeId) return;
    if (isCompacting) return;
    setCompactCompleteVisible(false);
    setIsCompacting(true);
    try {
      await api.compactConversation(chat.activeId);
      await chat.reloadMessages({ resetUsage: true });
      setCompactCompleteVisible(true);
    } catch (e) {
      toast.error(formatUserError(t('chat.compact'), e));
    } finally {
      setIsCompacting(false);
    }
  }, [chat.activeId, chat.reloadMessages, isCompacting, t]);

  useEffect(() => {
    if (chat.isStreaming) {
      setCompactCompleteVisible(false);
    }
  }, [chat.isStreaming]);

  useEffect(() => {
    setCompactCompleteVisible(false);
  }, [chat.activeId]);

  const pendingChatAction = (
    location.state as { pendingChatAction?: string } | null
  )?.pendingChatAction;

  useEffect(() => {
    if (pendingChatAction !== 'compact') return;
    if (!chat.activeId || chat.loadingMsgs || isCompacting) return;

    navigate(location.pathname, { replace: true, state: null });
    void handleCompactConversation();
  }, [
    chat.activeId,
    chat.loadingMsgs,
    handleCompactConversation,
    isCompacting,
    location.pathname,
    navigate,
    pendingChatAction,
  ]);

  /* ── No provider configured ─────────────────────────────────────── */
  if (!chat.loadingConfig && !chat.agentConfig) {
    return (
      <div className="flex items-center justify-center h-full">
        <EmptyState
          icon={<><Logo size={48} className="mx-auto mb-2" /><Settings className="h-8 w-8" /></>}
          title={t('chat.noProvider')}
          description={t('chat.noProviderDesc')}
          action={{
            label: t('chat.configureProvider'),
            onClick: () => navigate('/settings'),
          }}
        />
      </div>
    );
  }

  /* ── Render ──────────────────────────────────────────────────────── */
  return (
    <div className="flex h-full min-h-0">
      {/* Sidebar */}
      <motion.div
        initial={false}
        animate={{ width: sidebarCollapsed ? 0 : chatSidebarWidth }}
        transition={isChatSidebarResizing ? { duration: 0 } : { duration: 0.2, ease: [0.16, 1, 0.3, 1] }}
        className="relative shrink-0 overflow-hidden h-full min-h-0"
      >
        <div className="h-full min-h-0" style={{ width: chatSidebarWidth }}>
          <ChatSidebar
            conversations={chat.conversations}
            activeId={chat.activeId}
            runningConversationIds={chat.runningConversationIds}
            activeConversationArchived={isArchivedConversation}
            onSelect={handleSelectConversation}
            onNew={handleNewConversation}
            onArchive={handleArchiveConversation}
            onDelete={handleDeleteConversation}
            onRename={chat.renameConversation}
            onDeleteBatch={handleDeleteBatch}
            onDeleteAll={handleDeleteAll}
            onSelectArchived={handleSelectArchivedConversation}
            onArchivedRestored={handleArchivedConversationRestored}
            onArchivedDeleted={handleArchivedConversationDeleted}
            onConversationMoved={chat.loadConversations}
          />
        </div>
        {!sidebarCollapsed && (
          <div
            role="separator"
            aria-orientation="vertical"
            aria-valuemin={CHAT_SIDEBAR_MIN_WIDTH}
            aria-valuemax={CHAT_SIDEBAR_MAX_WIDTH}
            aria-valuenow={chatSidebarWidth}
            tabIndex={0}
            onPointerDown={startChatSidebarResize}
            onKeyDown={handleChatSidebarResizeKey}
            className="absolute right-0 top-0 h-full w-2 translate-x-1 cursor-col-resize touch-none
              bg-transparent outline-none transition-colors hover:bg-accent/25 focus-visible:bg-accent/35"
            title={t('nav.resizeSidebar')}
          />
        )}
      </motion.div>

      {/* Main chat area */}
      <div className="flex-1 flex flex-col min-w-0 min-h-0 relative">
        {!chat.activeId && (
          <div className="absolute top-2 left-2 z-20">
            <button
              type="button"
              onClick={toggleSidebar}
              className="p-1.5 rounded-md bg-surface-2/80 backdrop-blur border border-border/50
                text-text-tertiary hover:text-text-primary hover:bg-surface-3
                transition-colors cursor-pointer"
              title={t('chat.toggleSidebar')}
              aria-label={t('chat.toggleSidebar')}
            >
              {sidebarCollapsed ? <PanelLeftOpen size={16} /> : <PanelLeftClose size={16} />}
            </button>
          </div>
        )}
        {!chat.activeId && !chat.isStreaming && !pendingGraphContext ? (
          <div className="flex-1 flex items-center justify-center">
            <EmptyState
              icon={<Logo size={64} />}
              title={t('chat.noConversations')}
              description={t('chat.noConversationsDesc')}
              action={{
                label: t('chat.newChat'),
                onClick: () => handleNewConversation(),
              }}
            />
          </div>
        ) : (
          <>
            {chat.activeId && (
              <div className="sticky top-0 z-10 shrink-0 border-b border-border/60 bg-surface-1/85 px-3 py-1.5 backdrop-blur">
                <div className="flex min-h-10 flex-wrap items-center gap-2">
                  <button
                    type="button"
                    onClick={toggleSidebar}
                    className="flex h-9 w-9 shrink-0 items-center justify-center rounded-md border border-border/50 bg-surface-2/70
                      text-text-tertiary hover:text-text-primary hover:bg-surface-3
                      transition-colors cursor-pointer"
                    title={t('chat.toggleSidebar')}
                    aria-label={t('chat.toggleSidebar')}
                  >
                    {sidebarCollapsed ? <PanelLeftOpen size={16} /> : <PanelLeftClose size={16} />}
                  </button>
                  {isArchivedConversation ? (
                    <div
                      className="flex min-w-0 flex-1 items-center gap-2"
                      data-testid="archived-conversation-banner"
                    >
                      <Archive className="h-4 w-4 shrink-0 text-text-tertiary" />
                      <div className="min-w-0 flex-1">
                        <div className="truncate text-xs font-medium text-text-primary">
                          {chat.activeConversation?.title || t('chat.newConversation')}
                        </div>
                        <div className="truncate text-[10px] text-text-tertiary">
                          {t('chat.archivedReadOnly')}
                        </div>
                      </div>
                      <Button
                        variant="secondary"
                        size="sm"
                        icon={restoringArchivedConversation
                          ? <Loader2 className="h-3.5 w-3.5 animate-spin" />
                          : <ArchiveRestore className="h-3.5 w-3.5" />}
                        disabled={restoringArchivedConversation}
                        onClick={() => void handleRestoreActiveArchivedConversation()}
                      >
                        {t('chat.restoreConversation')}
                      </Button>
                    </div>
                  ) : (
                    <>
                      <div className="flex min-w-0 flex-1 flex-wrap items-center justify-start gap-2">
                        <SourceSelector
                          conversationId={chat.activeId}
                          initialSelectedIds={initialSourceIds}
                          onSelectionChange={handleSourceSelectionChange}
                        />
                        <SystemPromptEditor
                          conversationId={chat.activeId}
                          systemPrompt={chat.customSystemPrompt}
                          onSaved={(newPrompt) => chat.setCustomSystemPrompt(newPrompt)}
                        />
                        <ActiveExtensions
                          conversationId={chat.activeId ?? undefined}
                        />
                      </div>
                      <button
                        type="button"
                        data-testid="chat-auto-tts-toggle"
                        onClick={() => void toggleAutoSpeech()}
                        disabled={!appConfig?.textToSpeech || autoSpeechSaving}
                        aria-pressed={autoSpeechEnabled}
                        className={`flex h-9 w-9 shrink-0 items-center justify-center rounded-md border transition-colors disabled:pointer-events-none disabled:opacity-40 ${
                          autoSpeechEnabled
                            ? 'border-accent/35 bg-accent/10 text-accent hover:bg-accent/15'
                            : 'border-border/50 bg-surface-2/70 text-text-tertiary hover:bg-surface-3 hover:text-text-primary'
                        }`}
                        title={autoSpeechEnabled ? t('chat.autoTtsOn') : t('chat.autoTtsOff')}
                        aria-label={autoSpeechEnabled ? t('chat.autoTtsOn') : t('chat.autoTtsOff')}
                      >
                        {autoSpeechSaving
                          ? <Loader2 size={16} className="animate-spin" />
                          : autoSpeechEnabled
                            ? <Volume2 size={16} />
                            : <VolumeX size={16} />}
                      </button>
                      <button
                        type="button"
                        data-testid="browser-workspace-toggle"
                        onClick={handleToggleBrowser}
                        className={`relative flex h-9 w-9 shrink-0 items-center justify-center rounded-md border transition-colors cursor-pointer ${
                          browserOpen
                            ? 'border-cyan-400/35 bg-cyan-400/10 text-cyan-300'
                            : 'border-border/50 bg-surface-2/70 text-text-tertiary hover:bg-surface-3 hover:text-text-primary'
                        }`}
                        title={`${t('browser.title')} (Ctrl+Shift+B / Cmd+Shift+B)`}
                        aria-label={t('browser.title')}
                        aria-keyshortcuts="Control+Shift+B Meta+Shift+B"
                        aria-pressed={browserOpen}
                      >
                        <Globe2 size={16} />
                        {browserStatus.state !== 'empty' && (
                          <span className={`absolute right-1 top-1 h-1.5 w-1.5 rounded-full ${
                            browserStatus.state === 'agent' || browserStatus.state === 'loading'
                              ? 'animate-pulse bg-cyan-300'
                              : browserStatus.state === 'error'
                                ? 'bg-danger'
                                : browserStatus.state === 'user'
                                  ? 'bg-emerald-400'
                                  : 'bg-blue-400'
                          }`} />
                        )}
                        {browserStatus.tabCount > 1 && (
                          <span className="absolute -right-1 -top-1 grid min-h-4 min-w-4 place-items-center rounded-full border border-surface-1 bg-cyan-500 px-1 text-[8px] font-semibold text-slate-950">
                            {browserStatus.tabCount}
                          </span>
                        )}
                      </button>
                      <button
                        type="button"
                        onClick={handleToggleTerminal}
                        className="flex h-9 w-9 shrink-0 items-center justify-center rounded-md border border-border/50 bg-surface-2/70
                          text-text-tertiary hover:text-text-primary hover:bg-surface-3
                          transition-colors cursor-pointer"
                        title={`${t('shortcuts.toggleTerminal')} (Ctrl+J / Cmd+J)`}
                        aria-label={t('shortcuts.toggleTerminal')}
                        aria-keyshortcuts="Control+J Meta+J"
                      >
                        <TerminalSquare size={16} />
                      </button>
                    </>
                  )}
                </div>
              </div>
            )}
            <ChatMessages
              conversationId={chat.activeId}
              messages={chat.messages}
              turns={chat.turns}
              streamText={chat.streamText}
              streamRounds={chat.streamRounds}
              traceEvents={chat.traceEvents}
              thinkingText={chat.thinkingText}
              isThinking={chat.isThinking}
              toolCalls={chat.toolCalls}
              taskRun={chat.taskRun}
              isStreaming={chat.isStreaming}
              error={chat.error}
              onRetry={isArchivedConversation ? undefined : chat.retry}
              onDismissError={() => chat.clearError()}
              onDeleteMessage={isArchivedConversation ? undefined : chat.deleteMessage}
              onEditAndResend={isArchivedConversation ? undefined : chat.editAndResend}
              onApprovePlan={isArchivedConversation ? undefined : handleApprovePlan}
              onQuestionSubmit={isArchivedConversation ? undefined : (message, artifact) => {
                void handleChatSend(message, undefined, { userArtifacts: artifact });
              }}
              loadingMsgs={chat.loadingMsgs}
              lastCached={chat.lastCached}
              onSuggestionClick={isArchivedConversation ? undefined : handleSuggestionClick}
              isCompacting={isCompacting}
              compactCompleteVisible={compactCompleteVisible}
            />
            <TaskBoard
              messages={chat.messages}
              toolCalls={chat.toolCalls}
              taskRun={chat.taskRun}
              taskEvents={chat.taskEvents}
              goal={activeGoalContext}
            />
            {!isArchivedConversation && (
              <TerminalDock
                conversationId={chat.activeId ?? undefined}
                agentLabel={selectedAgentConfig?.name || chat.agentConfig?.model}
                onSendSelectionToAgent={handleTerminalSelection}
              />
            )}
            {pendingGraphContext && (
              <div className="mx-4 mb-2 rounded-md border border-accent/25 bg-accent/10 px-3 py-2">
                <div className="flex min-w-0 items-center gap-2">
                  <Network className="h-4 w-4 shrink-0 text-accent" />
                  <div className="min-w-0 flex-1">
                    <div className="truncate text-xs font-medium text-text-primary">
                      {t('chat.graphContextFromKnowledge', { name: pendingGraphContext.node.label })}
                    </div>
                    <div className="mt-0.5 truncate text-[11px] text-text-tertiary">
                      {[
                        pendingGraphContext.sourceLabel,
                        pendingGraphContext.pathPrefix,
                        t('chat.graphContextStats', {
                          nodes: String(new Set([
                            pendingGraphContext.node.id,
                            ...pendingGraphContext.edges.flatMap((edge) => [edge.source, edge.target]),
                          ]).size),
                          documents: String(pendingGraphContext.documents.length),
                          saved: String(pendingGraphContext.tokenEstimate.savedPctEstimate),
                        }),
                      ].filter(Boolean).join(' · ')}
                    </div>
                  </div>
                  <Button
                    variant="ghost"
                    size="sm"
                    iconOnly
                    icon={<X size={14} />}
                    aria-label={t('chat.removeGraphContext')}
                    title={t('chat.removeGraphContext')}
                    onClick={handleClearGraphContext}
                  />
                </div>
              </div>
            )}
            {isArchivedConversation ? (
              <div className="shrink-0 border-t border-border/70 bg-surface-1 px-4 py-3">
                <div className="mx-auto flex max-w-4xl items-center justify-between gap-3 rounded-lg border border-border/70 bg-surface-2/70 px-3 py-2">
                  <div className="min-w-0">
                    <div className="text-xs font-medium text-text-primary">{t('chat.archivedReadOnly')}</div>
                    <div className="text-[11px] text-text-tertiary">{t('chat.restoreToContinue')}</div>
                  </div>
                  <Button
                    variant="secondary"
                    size="sm"
                    icon={restoringArchivedConversation
                      ? <Loader2 className="h-3.5 w-3.5 animate-spin" />
                      : <ArchiveRestore className="h-3.5 w-3.5" />}
                    disabled={restoringArchivedConversation}
                    onClick={() => void handleRestoreActiveArchivedConversation()}
                  >
                    {t('chat.restoreConversation')}
                  </Button>
                </div>
              </div>
            ) : <ChatInput
              onSend={handleChatSend}
              onStop={chat.stop}
              isStreaming={chat.isStreaming}
              disabled={!chat.agentConfig || chat.loadingMsgs}
              conversationId={chat.activeId ?? undefined}
              inputHistory={chatInputHistory}
              sessionControls={sessionControls}
              prefillText={prefillText}
              prefillKey={prefillKey}
              onCompact={chat.activeId ? handleCompactConversation : undefined}
              isCompacting={isCompacting}
              planModeEnabled={planModeEnabled}
              onPlanModeChange={setPlanModeEnabled}
              activeGoalContext={activeGoalContext}
              contextIndicator={chat.activeId ? (
                <ChatRunOverview
                  isStreaming={chat.isStreaming}
                  tokenUsage={chat.tokenUsage}
                  runtimeProfile={chat.runtimeProfile}
                  finishReason={chat.finishReason}
                  contextOverflow={chat.contextOverflow}
                  isCompacting={isCompacting}
                />
              ) : null}
              onRestoreCheckpoint={chat.activeId ? async () => {
                await chat.reloadMessages();
              } : undefined}
              onBranchCheckpoint={handleCheckpointBranch}
            />}
            {chat.activeId && !isArchivedConversation && (
              <ApprovalDialogMount conversationId={chat.activeId} />
            )}
          </>
        )}
      </div>
      {!isArchivedConversation && (
        <BrowserDock
          open={browserOpen}
          conversationId={chat.activeId ?? undefined}
          agentLabel={selectedAgentConfig?.name || chat.agentConfig?.model}
          onOpenChange={setBrowserOpen}
          onStatusChange={setBrowserStatus}
          onSendArtifactToAgent={handleBrowserArtifact}
        />
      )}
    </div>
  );
}

export default ChatPage;

/**
 * Small wrapper that subscribes to the approval queue for the active
 * conversation and renders the modal dialog for the head request.
 */
function ApprovalDialogMount({ conversationId }: { conversationId: string }) {
  const { current, onResolved } = useApprovalQueue(conversationId);
  return <ApprovalDialog request={current} onResolved={onResolved} />;
}
