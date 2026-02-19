import { useState, useMemo, useCallback, useEffect } from 'react';
import { motion, AnimatePresence } from 'framer-motion';
import { Plus, Trash2, Pencil, MessageCircle, Check, X, Search, Star } from 'lucide-react';
import { useTranslation } from '../../i18n';
import type { TranslationKey } from '../../i18n';
import { Button } from '../ui/Button';
import { Badge } from '../ui/Badge';
import { EmptyState } from '../ui/EmptyState';
import { ConfirmDialog } from '../ui/ConfirmDialog';
import type { Conversation } from '../../types/conversation';

/* ------------------------------------------------------------------ */
/*  Types                                                              */
/* ------------------------------------------------------------------ */

interface ChatSidebarProps {
  conversations: Conversation[];
  activeId: string | null;
  onSelect: (id: string) => void;
  onNew: () => void;
  onDelete: (id: string) => void;
  onRename: (id: string, title: string) => void;
}

type TimeGroup = 'pinned' | 'today' | 'yesterday' | 'last7Days' | 'last30Days' | 'older';

interface GroupedConversations {
  key: TimeGroup;
  label: TranslationKey;
  conversations: Conversation[];
}

/* ------------------------------------------------------------------ */
/*  Helpers                                                            */
/* ------------------------------------------------------------------ */

const PINNED_STORAGE_KEY = 'chat-pinned-conversations';

function getPinnedIds(): Set<string> {
  try {
    const raw = localStorage.getItem(PINNED_STORAGE_KEY);
    if (raw) return new Set(JSON.parse(raw) as string[]);
  } catch { /* ignore */ }
  return new Set();
}

function savePinnedIds(ids: Set<string>) {
  localStorage.setItem(PINNED_STORAGE_KEY, JSON.stringify([...ids]));
}

function relativeTime(iso: string, t: (key: TranslationKey) => string): string {
  const diff = Date.now() - new Date(iso).getTime();
  const secs = Math.floor(diff / 1000);
  if (secs < 60) return t('time.justNow');
  const mins = Math.floor(secs / 60);
  if (mins < 60) return `${mins}${t('time.minuteShort')}`;
  const hrs = Math.floor(mins / 60);
  if (hrs < 24) return `${hrs}${t('time.hourShort')}`;
  const days = Math.floor(hrs / 24);
  if (days < 30) return `${days}${t('time.dayShort')}`;
  const months = Math.floor(days / 30);
  return `${months}${t('time.monthShort')}`;
}

function getTimeGroup(iso: string): TimeGroup {
  const now = new Date();
  const date = new Date(iso);

  const startOfToday = new Date(now.getFullYear(), now.getMonth(), now.getDate());
  const startOfYesterday = new Date(startOfToday.getTime() - 86_400_000);
  const startOf7Days = new Date(startOfToday.getTime() - 7 * 86_400_000);
  const startOf30Days = new Date(startOfToday.getTime() - 30 * 86_400_000);

  if (date >= startOfToday) return 'today';
  if (date >= startOfYesterday) return 'yesterday';
  if (date >= startOf7Days) return 'last7Days';
  if (date >= startOf30Days) return 'last30Days';
  return 'older';
}

const GROUP_ORDER: TimeGroup[] = ['pinned', 'today', 'yesterday', 'last7Days', 'last30Days', 'older'];

const GROUP_LABELS: Record<TimeGroup, TranslationKey> = {
  pinned: 'chat.pinned',
  today: 'chat.today',
  yesterday: 'chat.yesterday',
  last7Days: 'chat.last7Days',
  last30Days: 'chat.last30Days',
  older: 'chat.older',
};

function groupConversations(
  conversations: Conversation[],
  pinnedIds: Set<string>,
): GroupedConversations[] {
  const buckets: Record<TimeGroup, Conversation[]> = {
    pinned: [],
    today: [],
    yesterday: [],
    last7Days: [],
    last30Days: [],
    older: [],
  };

  for (const conv of conversations) {
    if (pinnedIds.has(conv.id)) {
      buckets.pinned.push(conv);
    } else {
      buckets[getTimeGroup(conv.updatedAt)].push(conv);
    }
  }

  return GROUP_ORDER
    .filter((key) => buckets[key].length > 0)
    .map((key) => ({ key, label: GROUP_LABELS[key], conversations: buckets[key] }));
}

const listItemVariants = {
  hidden: { opacity: 0, x: -12 },
  visible: (i: number) => ({
    opacity: 1,
    x: 0,
    transition: { delay: i * 0.03, duration: 0.2, ease: [0.16, 1, 0.3, 1] as const },
  }),
  exit: { opacity: 0, x: -12, transition: { duration: 0.15 } },
};

/* ------------------------------------------------------------------ */
/*  Conversation Item                                                  */
/* ------------------------------------------------------------------ */

function ConversationItem({
  conv,
  isActive,
  isPinned,
  index,
  onSelect,
  onDelete,
  onRename,
  onTogglePin,
  t,
}: {
  conv: Conversation;
  isActive: boolean;
  isPinned: boolean;
  index: number;
  onSelect: () => void;
  onDelete: () => void;
  onRename: (title: string) => void;
  onTogglePin: () => void;
  t: (key: TranslationKey) => string;
}) {
  const [hovered, setHovered] = useState(false);
  const [editing, setEditing] = useState(false);
  const [editTitle, setEditTitle] = useState('');

  const startRename = () => {
    setEditTitle(conv.title || '');
    setEditing(true);
  };

  const commitRename = () => {
    const trimmed = editTitle.trim();
    if (trimmed && trimmed !== conv.title) {
      onRename(trimmed);
    }
    setEditing(false);
  };

  return (
    <motion.div
      custom={index}
      variants={listItemVariants}
      initial="hidden"
      animate="visible"
      exit="exit"
      role="button"
      tabIndex={0}
      onMouseEnter={() => setHovered(true)}
      onMouseLeave={() => setHovered(false)}
      onClick={() => !editing && onSelect()}
      onKeyDown={(e) => { if (e.key === 'Enter' || e.key === ' ') { e.preventDefault(); if (!editing) onSelect(); } }}
      className={`group relative flex items-center gap-2 rounded-md px-2.5 py-2 cursor-pointer
        transition-colors duration-fast ease-out text-sm
        ${isActive
          ? 'bg-accent-subtle text-accent-hover'
          : 'text-text-secondary hover:bg-surface-2 hover:text-text-primary'
        }`}
    >
      {/* Active indicator */}
      {isActive && (
        <motion.span
          className="absolute left-0 top-1/2 -translate-y-1/2 w-[3px] rounded-r-full bg-accent"
          layoutId="chat-active-indicator"
          initial={false}
          animate={{ height: 20, opacity: 1 }}
          transition={{ duration: 0.25, ease: [0.16, 1, 0.3, 1] }}
        />
      )}

      <div className="flex-1 min-w-0">
        {editing ? (
          <div className="flex items-center gap-1" onClick={(e) => e.stopPropagation()}>
            <input
              autoFocus
              value={editTitle}
              onChange={(e) => setEditTitle(e.target.value)}
              onKeyDown={(e) => {
                if (e.key === 'Enter') commitRename();
                if (e.key === 'Escape') setEditing(false);
              }}
              className="flex-1 bg-surface-0 border border-border rounded px-1.5 py-0.5 text-xs
                text-text-primary outline-none focus:border-accent"
            />
            <button onClick={commitRename} className="text-success hover:text-success/80 cursor-pointer"
              aria-label={t('common.confirm')}
            >
              <Check className="h-3.5 w-3.5" />
            </button>
            <button onClick={() => setEditing(false)} className="text-text-tertiary hover:text-text-secondary cursor-pointer"
              aria-label={t('common.cancel')}
            >
              <X className="h-3.5 w-3.5" />
            </button>
          </div>
        ) : (
          <>
            <div className="truncate text-xs font-medium">
              {conv.title || t('chat.newConversation')}
            </div>
            <div className="flex items-center gap-1.5 mt-0.5">
              <Badge className="!text-[10px] !px-1.5">{conv.model}</Badge>
              <span className="text-[10px] text-text-tertiary">{relativeTime(conv.updatedAt, t)}</span>
            </div>
          </>
        )}
      </div>

      {/* Hover actions */}
      {(hovered || isPinned) && !editing && (
        <div className="flex items-center gap-0.5 shrink-0" onClick={(e) => e.stopPropagation()}>
          <button
            onClick={onTogglePin}
            className={`p-1 rounded transition-colors cursor-pointer ${
              isPinned
                ? 'text-warning hover:text-warning/70'
                : 'text-text-tertiary hover:text-warning'
            } ${!hovered && isPinned ? '' : 'hover:bg-surface-3'}`}
            aria-label={t('chat.pinned')}
          >
            <Star className={`h-3 w-3 ${isPinned ? 'fill-current' : ''}`} />
          </button>
          {hovered && (
            <>
              <button
                onClick={startRename}
                className="p-1 rounded hover:bg-surface-3 text-text-tertiary hover:text-text-secondary
                  transition-colors cursor-pointer"
                aria-label={t('common.edit')}
              >
                <Pencil className="h-3 w-3" />
              </button>
              <button
                onClick={onDelete}
                className="p-1 rounded hover:bg-danger/10 text-text-tertiary hover:text-danger
                  transition-colors cursor-pointer"
                aria-label={t('common.delete')}
              >
                <Trash2 className="h-3 w-3" />
              </button>
            </>
          )}
        </div>
      )}
    </motion.div>
  );
}

/* ------------------------------------------------------------------ */
/*  Component                                                          */
/* ------------------------------------------------------------------ */

export function ChatSidebar({
  conversations,
  activeId,
  onSelect,
  onNew,
  onDelete,
  onRename,
}: ChatSidebarProps) {
  const { t } = useTranslation();
  const [deleteTarget, setDeleteTarget] = useState<string | null>(null);
  const [searchQuery, setSearchQuery] = useState('');
  const [pinnedIds, setPinnedIds] = useState<Set<string>>(getPinnedIds);

  // Persist pinned state
  useEffect(() => {
    savePinnedIds(pinnedIds);
  }, [pinnedIds]);

  const togglePin = useCallback((id: string) => {
    setPinnedIds((prev) => {
      const next = new Set(prev);
      if (next.has(id)) next.delete(id);
      else next.add(id);
      return next;
    });
  }, []);

  // Filter by search query
  const filtered = useMemo(() => {
    if (!searchQuery.trim()) return conversations;
    const q = searchQuery.toLowerCase();
    return conversations.filter((c) =>
      (c.title || '').toLowerCase().includes(q),
    );
  }, [conversations, searchQuery]);

  // Group filtered conversations
  const groups = useMemo(() => groupConversations(filtered, pinnedIds), [filtered, pinnedIds]);

  // Running index for stagger animation across groups
  let runningIndex = 0;

  return (
    <div className="flex flex-col h-full bg-surface-1 border-r border-border">
      {/* Header */}
      <div className="flex items-center justify-between px-3 py-3 border-b border-border">
        <div className="flex items-center gap-1.5">
          <h2 className="text-xs font-semibold text-text-primary uppercase tracking-wider">
            {t('chat.title')}
          </h2>
          {conversations.length > 0 && (
            <Badge className="!text-[10px] !px-1.5">{conversations.length}</Badge>
          )}
        </div>
        <Button variant="ghost" size="sm" icon={<Plus className="h-3.5 w-3.5" />} onClick={onNew}>
          {t('chat.newChat')}
        </Button>
      </div>

      {/* Search bar */}
      {conversations.length > 0 && (
        <div className="px-2 py-2 border-b border-border">
          <div className="relative">
            <Search className="absolute left-2 top-1/2 -translate-y-1/2 h-3.5 w-3.5 text-text-tertiary pointer-events-none" />
            <input
              type="text"
              value={searchQuery}
              onChange={(e) => setSearchQuery(e.target.value)}
              placeholder={t('chat.searchConversations')}
              className="w-full bg-surface-0 border border-border rounded-md pl-7 pr-7 py-1.5 text-xs
                text-text-primary placeholder:text-text-tertiary outline-none
                focus:border-accent transition-colors"
            />
            {searchQuery && (
              <button
                onClick={() => setSearchQuery('')}
                className="absolute right-2 top-1/2 -translate-y-1/2 text-text-tertiary
                  hover:text-text-secondary cursor-pointer"
                aria-label={t('common.clear')}
              >
                <X className="h-3.5 w-3.5" />
              </button>
            )}
          </div>
        </div>
      )}

      {/* Conversation list */}
      <div className="flex-1 overflow-y-auto px-1.5 py-1.5">
        {conversations.length === 0 ? (
          <EmptyState
            icon={<MessageCircle className="h-6 w-6" />}
            title={t('chat.noConversations')}
            description={t('chat.noConversationsDesc')}
          />
        ) : filtered.length === 0 ? (
          <EmptyState
            icon={<Search className="h-6 w-6" />}
            title={t('chat.noSearchResults')}
            description=""
          />
        ) : (
          groups.map((group) => {
            const groupItems = group.conversations;
            const startIdx = runningIndex;
            runningIndex += groupItems.length;
            return (
              <div key={group.key} className="mb-2">
                <div className="flex items-center gap-1.5 px-2 pt-2 pb-1">
                  {group.key === 'pinned' && (
                    <Star className="h-3 w-3 text-warning fill-warning" />
                  )}
                  <span className="text-[10px] font-semibold text-text-tertiary uppercase tracking-wider">
                    {t(group.label)}
                  </span>
                </div>
                <AnimatePresence initial={false}>
                  {groupItems.map((conv, idx) => (
                    <ConversationItem
                      key={conv.id}
                      conv={conv}
                      isActive={conv.id === activeId}
                      isPinned={pinnedIds.has(conv.id)}
                      index={startIdx + idx}
                      onSelect={() => onSelect(conv.id)}
                      onDelete={() => setDeleteTarget(conv.id)}
                      onRename={(title) => onRename(conv.id, title)}
                      onTogglePin={() => togglePin(conv.id)}
                      t={t}
                    />
                  ))}
                </AnimatePresence>
              </div>
            );
          })
        )}
      </div>

      {/* Delete confirm */}
      <ConfirmDialog
        open={deleteTarget !== null}
        onClose={() => setDeleteTarget(null)}
        onConfirm={() => {
          if (deleteTarget) {
            onDelete(deleteTarget);
            setDeleteTarget(null);
          }
        }}
        title={t('chat.deleteConfirm')}
        message={t('chat.deleteConfirmDesc')}
      />
    </div>
  );
}
