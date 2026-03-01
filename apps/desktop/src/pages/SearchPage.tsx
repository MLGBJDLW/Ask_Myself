import { useState, useEffect, useCallback, useRef } from 'react';
import { useNavigate } from 'react-router-dom';
import { motion, AnimatePresence } from 'framer-motion';
import {
  Search,
  AlertTriangle,
  BookmarkPlus,
  Clock,
  Filter,
  X,
  Trash2,
  BotMessageSquare,
  ChevronLeft,
  ChevronRight,
  ChevronDown,
  Database,
  FileText,
  Layers,
  FolderOpen,
  ExternalLink,
} from 'lucide-react';
import { Logo } from '../components/Logo';
import { toast } from 'sonner';
import * as api from '../lib/api';
import type {
  SearchResult,
  QueryLog,
  Feedback,
  Source,
  Playbook,
  SearchFilters,
  IndexStats,
} from '../types';
import type { FileType } from '../types/document';
import { EvidenceCardComponent } from '../components/EvidenceCard';
import { Input } from '../components/ui/Input';
import { Button } from '../components/ui/Button';
import { Badge } from '../components/ui/Badge';
import { CardSkeleton } from '../components/ui/Skeleton';
import { Modal } from '../components/ui/Modal';
import { EmptyState } from '../components/ui/EmptyState';
import { Tooltip } from '../components/ui/Tooltip';
import { useTranslation } from '../i18n';

/* ------------------------------------------------------------------ */
/*  Constants                                                          */
/* ------------------------------------------------------------------ */

const PAGE_SIZE = 20;

const FILE_TYPE_OPTIONS: { value: FileType; label: string; labelKey: 'search.markdown' | 'search.plaintext' | 'search.log' | 'search.pdf' | 'search.docx' | 'search.excel' | 'search.pptx' }[] = [
  { value: 'markdown', label: 'Markdown', labelKey: 'search.markdown' },
  { value: 'plaintext', label: 'Plain text', labelKey: 'search.plaintext' },
  { value: 'log', label: 'Log', labelKey: 'search.log' },
  { value: 'pdf', label: 'PDF', labelKey: 'search.pdf' },
  { value: 'docx', label: 'Word Document', labelKey: 'search.docx' },
  { value: 'excel', label: 'Excel Spreadsheet', labelKey: 'search.excel' },
  { value: 'pptx', label: 'PowerPoint', labelKey: 'search.pptx' },
];

/* ------------------------------------------------------------------ */
/*  Component                                                          */
/* ------------------------------------------------------------------ */

export function SearchPage() {
  const { t } = useTranslation();
  const navigate = useNavigate();
  // 鈹€鈹€ Core search state 鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€
  const [query, setQuery] = useState('');
  const [result, setResult] = useState<SearchResult | null>(null);
  const [recentQueries, setRecentQueries] = useState<QueryLog[]>([]);
  const [loading, setLoading] = useState(false);
  const [searchMode, setSearchMode] = useState<'fts' | 'hybrid'>('fts');
  const [feedbackMap, setFeedbackMap] = useState<Record<string, Feedback>>({});
  const [currentPage, setCurrentPage] = useState(1);

  // 鈹€鈹€ Filters 鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€
  const [filtersOpen, setFiltersOpen] = useState(false);
  const [sources, setSources] = useState<Source[]>([]);
  const [filters, setFilters] = useState<SearchFilters>({
    sourceIds: [],
    fileTypes: [],
    dateFrom: null,
    dateTo: null,
  });

  // 鈹€鈹€ Save-to-playbook modal 鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€
  const [savingChunkId, setSavingChunkId] = useState<string | null>(null);
  const [playbooks, setPlaybooks] = useState<Playbook[]>([]);
  const [selectedPlaybookId, setSelectedPlaybookId] = useState('');
  const [newPlaybookTitle, setNewPlaybookTitle] = useState('');
  const [citationNote, setCitationNote] = useState('');
  const [saveLoading, setSaveLoading] = useState(false);

  // 鈹€鈹€ Refs 鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€
  const inputRef = useRef<HTMLInputElement>(null);

  // 鈹€鈹€ Knowledge Base stats 鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€
  const [indexStats, setIndexStats] = useState<IndexStats | null>(null);
  const [kbOpen, setKbOpen] = useState(true);

  // Navigate to full chat page (optional one-off initial message)
  const openChatWithMessage = useCallback((message: string) => {
    const trimmed = message.trim();
    navigate('/chat', {
      state: trimmed ? { initialMessage: trimmed } : null,
    });
  }, [navigate]);

  // Auto-focus + Ctrl/Cmd+K + Ctrl/Cmd+Shift+A shortcuts
  useEffect(() => {
    inputRef.current?.focus();

    const handleKeyDown = (e: KeyboardEvent) => {
      if ((e.ctrlKey || e.metaKey) && e.key === 'k') {
        e.preventDefault();
        inputRef.current?.focus();
        inputRef.current?.select();
      }
      if ((e.ctrlKey || e.metaKey) && e.shiftKey && (e.key === 'a' || e.key === 'A')) {
        e.preventDefault();
        openChatWithMessage(query);
      }
    };
    document.addEventListener('keydown', handleKeyDown);
    return () => document.removeEventListener('keydown', handleKeyDown);
  }, [openChatWithMessage, query]);

  // 鈹€鈹€ Load recent queries on mount 鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€
  const loadRecentQueries = useCallback(async () => {
    try {
      const recent = await api.getRecentQueries(10);
      setRecentQueries(recent);
    } catch {
      // non-critical
    }
  }, []);

  const clearRecentQueries = useCallback(async () => {
    try {
      await api.clearRecentQueries();
      setRecentQueries([]);
    } catch {
      toast.error(t('search.searchError'));
    }
  }, [t]);

  useEffect(() => {
    loadRecentQueries();
  }, [loadRecentQueries]);

  // 鈹€鈹€ Load sources for filters 鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€
  useEffect(() => {
    api.listSources().then(setSources).catch((e) => {
      console.error('Failed to load sources for filters:', e);
    });
    api.getIndexStats().then(setIndexStats).catch((e) => {
      console.error('Failed to load index stats:', e);
    });
  }, []);

  // 鈹€鈹€ Reset page when query or filters change 鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€
  useEffect(() => {
    setCurrentPage(1);
  }, [searchMode, filters]);

  // 鈹€鈹€ Search handler 鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€
  const handleSearch = async (text?: string, page?: number) => {
    const q = text ?? query;
    if (!q.trim()) return;
    const targetPage = page ?? 1;
    if (!page) setCurrentPage(1);
    setLoading(true);
    setFeedbackMap({});
    const offset = (targetPage - 1) * PAGE_SIZE;
    const apiFilters = {
      ...filters,
      dateFrom: filters.dateFrom ? filters.dateFrom + "T00:00:00Z" : null,
      dateTo: filters.dateTo ? filters.dateTo + "T23:59:59Z" : null,
    };
    try {
      const res =
        searchMode === 'hybrid'
          ? await api.hybridSearch(q.trim(), PAGE_SIZE, offset, filters)
          : await api.search(q.trim(), PAGE_SIZE, offset, apiFilters);
      setResult({ ...res, searchMode });
      loadRecentQueries();

      // Load existing feedback
      try {
        const feedbacks = await api.getFeedbackForQuery(q.trim());
        const map: Record<string, Feedback> = {};
        for (const fb of feedbacks) {
          map[fb.chunkId] = fb;
        }
        setFeedbackMap(map);
      } catch {
        // non-critical
      }
    } catch (e) {
      toast.error(`${t('search.searchError')}: ${String(e)}`);
    } finally {
      setLoading(false);
    }
  };

  // 鈹€鈹€ Feedback handler 鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€
  const handleFeedback = async (
    chunkId: string,
    action: 'upvote' | 'downvote' | 'pin',
  ) => {
    if (!result) return;
    try {
      const existing = feedbackMap[chunkId];
      if (existing && existing.action === action) {
        await api.deleteFeedback(existing.id);
        setFeedbackMap((prev) => {
          const next = { ...prev };
          delete next[chunkId];
          return next;
        });
        return;
      }
      if (existing) {
        await api.deleteFeedback(existing.id);
      }
      const fb = await api.addFeedback(chunkId, result.query, action);
      setFeedbackMap((prev) => ({ ...prev, [chunkId]: fb }));
    } catch (e) {
      toast.error(`${t('search.feedbackError')}: ${String(e)}`);
    }
  };

  // 鈹€鈹€ Open save modal 鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€
  const openSaveModal = async (chunkId: string) => {
    setSavingChunkId(chunkId);
    setSelectedPlaybookId('');
    setNewPlaybookTitle('');
    setCitationNote('');
    try {
      const pbs = await api.listPlaybooks();
      setPlaybooks(pbs);
      if (pbs.length > 0) setSelectedPlaybookId(pbs[0].id);
    } catch {
      setPlaybooks([]);
    }
  };

  // 鈹€鈹€ Confirm save to playbook 鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€
  const confirmSave = async () => {
    if (!savingChunkId) return;
    setSaveLoading(true);
    try {
      let targetId = selectedPlaybookId;

      // Create new playbook if needed
      if (!targetId && newPlaybookTitle.trim()) {
        const pb = await api.createPlaybook(
          newPlaybookTitle.trim(),
          '',
          result?.query ?? '',
        );
        targetId = pb.id;
      }

      if (!targetId) {
        toast.error(t('search.needSelectPlaybook'));
        setSaveLoading(false);
        return;
      }

      await api.addCitation(targetId, savingChunkId, citationNote, 0);
      toast.success(t('search.savedToPlaybook'));
      setSavingChunkId(null);
      setCitationNote('');
      setNewPlaybookTitle('');
    } catch (e) {
      toast.error(`${t('search.saveError')}: ${String(e)}`);
    } finally {
      setSaveLoading(false);
    }
  };

  // 鈹€鈹€ Filter helpers 鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€
  const toggleSourceFilter = (id: string) => {
    setFilters((prev) => ({
      ...prev,
      sourceIds: prev.sourceIds.includes(id)
        ? prev.sourceIds.filter((s) => s !== id)
        : [...prev.sourceIds, id],
    }));
  };

  const toggleFileTypeFilter = (ft: FileType) => {
    setFilters((prev) => ({
      ...prev,
      fileTypes: prev.fileTypes.includes(ft)
        ? prev.fileTypes.filter((f) => f !== ft)
        : [...prev.fileTypes, ft],
    }));
  };

  const activeFilterCount =
    filters.sourceIds.length + filters.fileTypes.length +
    (filters.dateFrom ? 1 : 0) + (filters.dateTo ? 1 : 0);

  // 鈹€鈹€ Uncertainty detection 鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€
  const showUncertainty =
    result !== null &&
    result.evidenceCards.length > 0 &&
    result.evidenceCards.length < 3 &&
    result.evidenceCards.every((c) => c.score < 1);

  // 鈹€鈹€ Render 鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€
  return (
    <div className="flex h-full">
      {/* 鈹€鈹€ Main search area 鈹€鈹€ */}
      <div className="flex-1 transition-all duration-300">
        <div className="mx-auto max-w-3xl px-6 py-8">
      {/* 鈹€鈹€ Header 鈹€鈹€ */}
      <div className="mb-8">
        <h1 className="mb-1.5 text-lg font-semibold text-text-primary">{t('nav.search')}</h1>
        <p className="text-xs text-text-tertiary">
          {t('search.subtitle')}
          <kbd className="ml-2 inline-flex items-center rounded border border-border bg-surface-3 px-1.5 py-0.5 font-mono text-[10px] text-text-tertiary">
            Ctrl+K
          </kbd>
        </p>
      </div>

      {/* 鈹€鈹€ Knowledge Base Overview 鈹€鈹€ */}
      {indexStats && (
        <div className="mb-6 rounded-lg border border-border bg-surface-1 overflow-hidden">
          <button
            onClick={() => setKbOpen(!kbOpen)}
            className="flex w-full items-center justify-between px-4 py-2.5 text-left hover:bg-surface-2 transition-colors duration-fast"
          >
            <div className="flex items-center gap-2">
              <Database size={14} className="text-accent" />
              <span className="text-xs font-semibold text-text-primary">{t('search.knowledgeBase')}</span>
            </div>
            <ChevronDown
              size={14}
              className={`text-text-tertiary transition-transform duration-200 ${kbOpen ? '' : '-rotate-90'}`}
            />
          </button>
          <AnimatePresence>
            {kbOpen && (
              <motion.div
                initial={{ height: 0, opacity: 0 }}
                animate={{ height: 'auto', opacity: 1 }}
                exit={{ height: 0, opacity: 0 }}
                transition={{ duration: 0.2, ease: [0.16, 1, 0.3, 1] }}
                className="overflow-hidden"
              >
                <div className="px-4 pb-3 pt-1">
                  {/* Stat cards */}
                  <div className="grid grid-cols-3 gap-3 mb-3">
                    <div className="rounded-md border border-border bg-surface-2 px-3 py-2 text-center">
                      <div className="flex items-center justify-center gap-1.5 mb-0.5">
                        <FolderOpen size={12} className="text-accent" />
                        <span className="text-[10px] font-medium text-text-tertiary">{t('search.totalSources')}</span>
                      </div>
                      <p className="text-lg font-bold text-text-primary">{indexStats.totalSources}</p>
                    </div>
                    <div className="rounded-md border border-border bg-surface-2 px-3 py-2 text-center">
                      <div className="flex items-center justify-center gap-1.5 mb-0.5">
                        <FileText size={12} className="text-accent" />
                        <span className="text-[10px] font-medium text-text-tertiary">{t('search.totalDocuments')}</span>
                      </div>
                      <p className="text-lg font-bold text-text-primary">{indexStats.totalDocuments}</p>
                    </div>
                    <div className="rounded-md border border-border bg-surface-2 px-3 py-2 text-center">
                      <div className="flex items-center justify-center gap-1.5 mb-0.5">
                        <Layers size={12} className="text-accent" />
                        <span className="text-[10px] font-medium text-text-tertiary">{t('search.totalChunks')}</span>
                      </div>
                      <p className="text-lg font-bold text-text-primary">{indexStats.totalChunks}</p>
                    </div>
                  </div>

                  {/* Source list */}
                  {sources.length > 0 ? (
                    <div className="space-y-1">
                      {sources.map((s) => (
                        <div
                          key={s.id}
                          className="flex items-center justify-between gap-2 rounded-md px-2.5 py-1.5 text-xs hover:bg-surface-2 transition-colors duration-fast"
                        >
                          <div className="flex items-center gap-2 min-w-0">
                            <FolderOpen size={12} className="text-text-tertiary shrink-0" />
                            <span className="truncate text-text-secondary font-mono text-[11px]">
                              {s.rootPath.split(/[/\\]/).pop()}
                            </span>
                          </div>
                          <Badge variant="default" className="shrink-0 text-[10px]">
                            {s.kind === 'local_folder' ? t('sources.addModal.kindFolder') : s.kind}
                          </Badge>
                        </div>
                      ))}
                    </div>
                  ) : (
                    <p className="text-[11px] text-text-tertiary text-center py-2">
                      {t('search.kbEmptyDesc')}
                    </p>
                  )}

                  {/* Manage sources link */}
                  <button
                    onClick={() => navigate('/sources')}
                    className="mt-2 flex items-center gap-1 text-[11px] text-accent hover:text-accent-hover transition-colors"
                  >
                    <ExternalLink size={10} />
                    {t('search.viewSources')}
                  </button>
                </div>
              </motion.div>
            )}
          </AnimatePresence>
        </div>
      )}

      {/* 鈹€鈹€ Search input 鈹€鈹€ */}
      <div className="mb-4">
        <div className="flex gap-2">
          <Input
            ref={inputRef}
            icon={<Search size={16} />}
            value={query}
            onChange={(e) => setQuery(e.target.value)}
            onKeyDown={(e) => e.key === 'Enter' && handleSearch()}
            placeholder={t('search.placeholder')}
            className="h-11"
          />
          <Button
            onClick={() => handleSearch()}
            loading={loading}
            icon={<Search size={14} />}
            size="lg"
          >
            {t('nav.search')}
          </Button>
          <Tooltip content={`${t('chat.askAi')} (Ctrl+Shift+A)`}>
            <Button
              variant="ghost"
              size="lg"
              icon={<BotMessageSquare size={16} />}
              onClick={() => {
                const msg = query.trim() || '';
                openChatWithMessage(msg);
              }}
              className="shrink-0"
            >
              {t('chat.askAi')}
            </Button>
          </Tooltip>
        </div>
      </div>

      {/* 鈹€鈹€ Mode toggle + filters row 鈹€鈹€ */}
      <div className="mb-6 flex items-center justify-between gap-3">
        {/* Pill toggle */}
        <div className="inline-flex rounded-full border border-border bg-surface-1 p-0.5">
          <button
            onClick={() => setSearchMode('fts')}
            aria-pressed={searchMode === 'fts'}
            className={`rounded-full px-4 py-1.5 text-xs font-medium transition-all duration-fast ${
              searchMode === 'fts'
                ? 'bg-accent text-white shadow-sm'
                : 'text-text-tertiary hover:text-text-secondary'
            }`}
          >
            {t('search.fts')}
          </button>
          <button
            onClick={() => setSearchMode('hybrid')}
            aria-pressed={searchMode === 'hybrid'}
            className={`rounded-full px-4 py-1.5 text-xs font-medium transition-all duration-fast ${
              searchMode === 'hybrid'
                ? 'bg-accent text-white shadow-sm'
                : 'text-text-tertiary hover:text-text-secondary'
            }`}
          >
            {t('search.hybrid')}
          </button>
        </div>

        {/* Filter toggle */}
        <Button
          variant={filtersOpen ? 'secondary' : 'ghost'}
          size="sm"
          icon={<Filter size={13} />}
          onClick={() => setFiltersOpen(!filtersOpen)}
        >
          {t('search.filters')}
          {activeFilterCount > 0 && (
            <Badge variant="info" className="ml-1">
              {activeFilterCount}
            </Badge>
          )}
        </Button>
      </div>

      {/* 鈹€鈹€ Collapsible filters 鈹€鈹€ */}
      <AnimatePresence>
        {filtersOpen && (
          <motion.div
            initial={{ height: 0, opacity: 0 }}
            animate={{ height: 'auto', opacity: 1 }}
            exit={{ height: 0, opacity: 0 }}
            transition={{ duration: 0.2, ease: [0.16, 1, 0.3, 1] }}
            className="overflow-hidden"
          >
            <div className="mb-6 rounded-lg border border-border bg-surface-1 p-4">
              <div className="flex items-center justify-between mb-3">
                <h3 className="text-xs font-semibold text-text-primary">{t('search.filterTitle')}</h3>
                {activeFilterCount > 0 && (
                  <button
                    onClick={() =>
                      setFilters({
                        sourceIds: [],
                        fileTypes: [],
                        dateFrom: null,
                        dateTo: null,
                      })
                    }
                    className="text-[11px] text-text-tertiary hover:text-text-secondary transition-colors"
                  >
                    {t('search.clearFilters')}
                  </button>
                )}
              </div>

              {/* Source filters */}
              {sources.length > 0 && (
                <div className="mb-3">
                  <label className="mb-1.5 block text-[11px] font-medium text-text-tertiary">
                    {t('search.sourceFilter')}
                  </label>
                  <div className="flex flex-wrap gap-1.5">
                    {sources.map((s) => {
                      const active = filters.sourceIds.includes(s.id);
                      return (
                        <button
                          key={s.id}
                          onClick={() => toggleSourceFilter(s.id)}
                          className={`inline-flex items-center gap-1 rounded-full border px-2.5 py-1 text-[11px] font-medium transition-all duration-fast ${
                            active
                              ? 'border-accent/40 bg-accent-subtle text-accent-hover'
                              : 'border-border bg-surface-2 text-text-tertiary hover:text-text-secondary hover:border-border-hover'
                          }`}
                        >
                          {active && <X size={10} />}
                          {s.rootPath.split(/[/\\]/).pop()}
                        </button>
                      );
                    })}
                  </div>
                </div>
              )}

              {/* Date range filters */}
              <div className="mb-3">
                <label className="mb-1.5 block text-[11px] font-medium text-text-tertiary">
                  {t('search.dateRange')}
                </label>
                <div className="flex flex-wrap items-center gap-2">
                  <div className="flex items-center gap-1.5">
                    <span className="text-[11px] text-text-tertiary">{t('search.dateFrom')}</span>
                    <div className="relative">
                      <input
                        type="date"
                        value={filters.dateFrom ?? ''}
                        onChange={(e) =>
                          setFilters((prev) => ({
                            ...prev,
                            dateFrom: e.target.value || null,
                          }))
                        }
                        className="rounded-full border border-border bg-surface-2 px-2.5 py-1 text-[11px] font-medium text-text-secondary transition-all duration-fast hover:border-border-hover focus:border-accent focus:outline-none"
                      />
                      {filters.dateFrom && (
                        <button
                          onClick={() =>
                            setFilters((prev) => ({ ...prev, dateFrom: null }))
                          }
                          className="absolute -right-1 -top-1 flex h-3.5 w-3.5 items-center justify-center rounded-full bg-accent text-white"
                        >
                          <X size={8} />
                        </button>
                      )}
                    </div>
                  </div>
                  <div className="flex items-center gap-1.5">
                    <span className="text-[11px] text-text-tertiary">{t('search.dateTo')}</span>
                    <div className="relative">
                      <input
                        type="date"
                        value={filters.dateTo ?? ''}
                        onChange={(e) =>
                          setFilters((prev) => ({
                            ...prev,
                            dateTo: e.target.value || null,
                          }))
                        }
                        className="rounded-full border border-border bg-surface-2 px-2.5 py-1 text-[11px] font-medium text-text-secondary transition-all duration-fast hover:border-border-hover focus:border-accent focus:outline-none"
                      />
                      {filters.dateTo && (
                        <button
                          onClick={() =>
                            setFilters((prev) => ({ ...prev, dateTo: null }))
                          }
                          className="absolute -right-1 -top-1 flex h-3.5 w-3.5 items-center justify-center rounded-full bg-accent text-white"
                        >
                          <X size={8} />
                        </button>
                      )}
                    </div>
                  </div>
                </div>
              </div>

              {/* File type filters */}
              <div>
                <label className="mb-1.5 block text-[11px] font-medium text-text-tertiary">
                  {t('search.fileTypeFilter')}
                </label>
                <div className="flex flex-wrap gap-1.5">
                  {FILE_TYPE_OPTIONS.map((ft) => {
                    const active = filters.fileTypes.includes(ft.value);
                    return (
                      <button
                        key={ft.value}
                        onClick={() => toggleFileTypeFilter(ft.value)}
                        className={`inline-flex items-center gap-1 rounded-full border px-2.5 py-1 text-[11px] font-medium transition-all duration-fast ${
                          active
                            ? 'border-accent/40 bg-accent-subtle text-accent-hover'
                            : 'border-border bg-surface-2 text-text-tertiary hover:text-text-secondary hover:border-border-hover'
                        }`}
                      >
                        {active && <X size={10} />}
                        {t(ft.labelKey)}
                      </button>
                    );
                  })}
                </div>
              </div>
            </div>
          </motion.div>
        )}
      </AnimatePresence>

      {/* 鈹€鈹€ Loading skeletons 鈹€鈹€ */}
      {loading && (
        <div className="space-y-3">
          <CardSkeleton />
          <CardSkeleton />
          <CardSkeleton />
        </div>
      )}

      {/* 鈹€鈹€ Results 鈹€鈹€ */}
      {result && !loading && (
        <>
          {/* Results header */}
          <div className="mb-4 flex items-center justify-between">
            <div className="flex items-center gap-2 text-xs text-text-tertiary">
              <span>
                {t('search.resultCount', { count: result.evidenceCards.length })}
                {result.evidenceCards.length !== result.totalMatches && (
                  <span className="text-text-tertiary">
                    {' '}
                    ({t('search.totalCount', { total: result.totalMatches })})
                  </span>
                )}
              </span>
              {result.searchMode && (
                <Badge variant={result.searchMode === 'hybrid' ? 'info' : 'default'}>
                  {result.searchMode === 'hybrid' ? t('search.hybrid') : t('search.fts')}
                </Badge>
              )}
            </div>
            <span className="text-[11px] text-text-tertiary">
              {result.searchTimeMs} {t('search.ms')}
            </span>
          </div>

          {/* Uncertainty banner */}
          {showUncertainty && (
            <motion.div
              initial={{ opacity: 0, y: -8 }}
              animate={{ opacity: 1, y: 0 }}
              className="mb-4 flex items-start gap-3 rounded-lg border border-warning/30 bg-warning/8 px-4 py-3"
            >
              <AlertTriangle
                size={16}
                className="mt-0.5 shrink-0 text-warning"
              />
              <div>
                <p className="text-sm font-medium text-warning">{t('search.uncertainty')}</p>
                <p className="mt-0.5 text-xs text-warning/70">
                  {t('search.uncertaintyDesc')}
                </p>
              </div>
            </motion.div>
          )}

          {/* Pagination info */}
          {result.totalMatches > PAGE_SIZE && (
            <div className="mb-3 text-xs text-text-tertiary">
              {t('search.showingResults', {
                from: String((currentPage - 1) * PAGE_SIZE + 1),
                to: String(Math.min(currentPage * PAGE_SIZE, result.totalMatches)),
                total: String(result.totalMatches),
              })}
            </div>
          )}

          {/* Evidence cards with staggered animation */}
          {result.evidenceCards.length > 0 ? (
            <>
            <motion.div
              className="space-y-3"
              initial="hidden"
              animate="visible"
              variants={{
                hidden: {},
                visible: { transition: { staggerChildren: 0.06 } },
              }}
            >
              {result.evidenceCards.map((card) => (
                <motion.div
                  key={card.chunkId}
                  variants={{
                    hidden: { opacity: 0, y: 16 },
                    visible: { opacity: 1, y: 0 },
                  }}
                  transition={{ duration: 0.35, ease: [0.16, 1, 0.3, 1] }}
                >
                  <EvidenceCardComponent
                    card={card}
                    onFeedback={handleFeedback}
                    feedbackState={{
                      upvoted: feedbackMap[card.chunkId]?.action === 'upvote',
                      downvoted:
                        feedbackMap[card.chunkId]?.action === 'downvote',
                      pinned: feedbackMap[card.chunkId]?.action === 'pin',
                    }}
                    onAskAbout={openChatWithMessage}
                  />

                  {/* Save-to-playbook action */}
                  <div className="mt-1.5 flex justify-end">
                    <button
                      onClick={() => openSaveModal(card.chunkId)}
                      className="inline-flex items-center gap-1 rounded-md px-2 py-1 text-[11px] text-text-tertiary transition-colors hover:bg-surface-2 hover:text-text-secondary"
                    >
                      <BookmarkPlus size={12} />
                      {t('search.saveToPlaybook')}
                    </button>
                  </div>
                </motion.div>
              ))}
            </motion.div>
            {/* Pagination controls */}
            {result.totalMatches > PAGE_SIZE && (() => {
              const totalPages = Math.ceil(result.totalMatches / PAGE_SIZE);
              return (
                <div className="mt-6 flex items-center justify-center gap-3">
                  <Button
                    variant="ghost"
                    size="sm"
                    icon={<ChevronLeft size={14} />}
                    disabled={currentPage <= 1}
                    onClick={() => {
                      const prev = currentPage - 1;
                      setCurrentPage(prev);
                      handleSearch(undefined, prev);
                    }}
                  >
                    {t('search.previous')}
                  </Button>
                  <span className="text-xs text-text-secondary">
                    {t('search.page')} {currentPage} {t('search.of')} {totalPages}
                  </span>
                  <Button
                    variant="ghost"
                    size="sm"
                    disabled={currentPage >= totalPages}
                    onClick={() => {
                      const next = currentPage + 1;
                      setCurrentPage(next);
                      handleSearch(undefined, next);
                    }}
                  >
                    {t('search.next')}
                    <ChevronRight size={14} className="ml-1" />
                  </Button>
                </div>
              );
            })()}
            </>
          ) : (
            <>
              {/* Uncertainty banner for zero results */}
              <motion.div
                initial={{ opacity: 0, y: -8 }}
                animate={{ opacity: 1, y: 0 }}
                className="mb-4 flex items-start gap-3 rounded-lg border border-warning/30 bg-warning/8 px-4 py-3"
              >
                <AlertTriangle
                  size={16}
                  className="mt-0.5 shrink-0 text-warning"
                />
                <div>
                  <p className="text-sm font-medium text-warning">{t('search.uncertainty')}</p>
                  <p className="mt-0.5 text-xs text-warning/70">
                    {t('search.uncertaintyDesc')}
                  </p>
                </div>
              </motion.div>
              <EmptyState
                icon={<Search size={32} />}
                title={t('search.noResults')}
                description={t('search.noResultsDesc', { query: result.query })}
              />
            </>
          )}
        </>
      )}

      {/* 鈹€鈹€ Recent queries (shown when no results/loading) 鈹€鈹€ */}
      {!result && !loading && recentQueries.length > 0 && (
        <div className="mt-2">
          <div className="mb-3 flex items-center justify-between">
            <div className="flex items-center gap-2 text-xs font-medium text-text-tertiary">
              <Clock size={12} />
              {t('search.recentQueries')}
            </div>
            <button
              onClick={clearRecentQueries}
              className="inline-flex items-center gap-1 rounded-md px-2 py-1 text-[11px] text-text-tertiary transition-colors hover:bg-surface-2 hover:text-text-secondary"
            >
              <Trash2 size={11} />
              {t('search.clearHistory')}
            </button>
          </div>
          <div className="flex flex-wrap gap-2">
            {recentQueries.map((q) => (
              <button
                key={q.id}
                onClick={() => {
                  setQuery(q.queryText);
                  handleSearch(q.queryText);
                }}
                className="group inline-flex items-center gap-2 rounded-full border border-border bg-surface-1 px-3 py-1.5 text-xs text-text-secondary transition-all duration-fast hover:border-border-hover hover:bg-surface-2 hover:text-text-primary"
              >
                <span className="truncate max-w-[200px]">{q.queryText}</span>
                <span className="shrink-0 text-[10px] text-text-tertiary group-hover:text-text-secondary">
                  {t('search.resultSuffix', { count: q.resultCount })}
                </span>
              </button>
            ))}
          </div>
        </div>
      )}

      {/* 鈹€鈹€ Initial empty state 鈹€鈹€ */}
      {!result && !loading && recentQueries.length === 0 && (
        <EmptyState
          icon={<Logo size={64} />}
          title={t('search.initialTitle')}
          description={t('search.initialDesc')}
        />
      )}

      {/* 鈹€鈹€ Save to Playbook modal 鈹€鈹€ */}
      <Modal
        open={savingChunkId !== null}
        onClose={() => {
          setSavingChunkId(null);
          setCitationNote('');
          setNewPlaybookTitle('');
        }}
        title={t('search.saveToPlaybook')}
        footer={
          <>
            <Button
              variant="ghost"
              size="sm"
              onClick={() => {
                setSavingChunkId(null);
                setCitationNote('');
                setNewPlaybookTitle('');
              }}
            >
              {t('common.cancel')}
            </Button>
            <Button
              size="sm"
              onClick={confirmSave}
              loading={saveLoading}
              disabled={!selectedPlaybookId && !newPlaybookTitle.trim()}
            >
              {t('common.save')}
            </Button>
          </>
        }
      >
        {playbooks.length > 0 ? (
          <div className="space-y-4">
            <div>
              <label className="mb-1.5 block text-xs font-medium text-text-secondary">
                {t('search.selectPlaybook')}
              </label>
              <select
                value={selectedPlaybookId}
                onChange={(e) => {
                  setSelectedPlaybookId(e.target.value);
                  if (e.target.value) setNewPlaybookTitle('');
                }}
                className="w-full rounded-md border border-border bg-surface-1 px-3 py-2 text-sm text-text-primary focus:border-accent focus:ring-1 focus:ring-accent/30 focus:outline-none"
              >
                {playbooks.map((pb) => (
                  <option key={pb.id} value={pb.id}>
                    {pb.title}
                  </option>
                ))}
                <option value="">+ {t('search.createNewPlaybook')}</option>
              </select>
            </div>

            {!selectedPlaybookId && (
              <div>
                <label className="mb-1.5 block text-xs font-medium text-text-secondary">
                  {t('search.newPlaybookName')}
                </label>
                <Input
                  value={newPlaybookTitle}
                  onChange={(e) => setNewPlaybookTitle(e.target.value)}
                  placeholder={t('search.newPlaybookName')}
                />
              </div>
            )}

            <div>
              <label className="mb-1.5 block text-xs font-medium text-text-secondary">
                {t('search.note')} <span className="text-text-tertiary">({t('search.optional')})</span>
              </label>
              <Input
                value={citationNote}
                onChange={(e) => setCitationNote(e.target.value)}
                placeholder={t('search.note')}
              />
            </div>
          </div>
        ) : (
          <div className="space-y-4">
            <p className="text-xs text-text-tertiary">
              {t('search.noPlaybooks')}
            </p>
            <div>
              <label className="mb-1.5 block text-xs font-medium text-text-secondary">
                {t('search.newPlaybookName')}
              </label>
              <Input
                value={newPlaybookTitle}
                onChange={(e) => setNewPlaybookTitle(e.target.value)}
                placeholder={t('search.newPlaybookName')}
              />
            </div>
            <div>
              <label className="mb-1.5 block text-xs font-medium text-text-secondary">
                {t('search.note')} <span className="text-text-tertiary">({t('search.optional')})</span>
              </label>
              <Input
                value={citationNote}
                onChange={(e) => setCitationNote(e.target.value)}
                placeholder={t('search.note')}
              />
            </div>
          </div>
        )}
      </Modal>
        </div>
      </div>

    </div>
  );
}


