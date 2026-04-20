import { useState, useEffect, useMemo, useRef, type ReactNode, type CSSProperties } from 'react';
import { NavLink, Outlet, useLocation, useNavigate } from 'react-router-dom';
import { motion, AnimatePresence, useReducedMotion } from 'framer-motion';
import { Search, FolderOpen, BookOpen, MessageCircle, Settings, ChevronLeft, ChevronRight, Brain, BotMessageSquare } from 'lucide-react';
import {
  DndContext,
  PointerSensor,
  KeyboardSensor,
  useSensor,
  useSensors,
  closestCenter,
  type DragEndEvent,
} from '@dnd-kit/core';
import {
  SortableContext,
  useSortable,
  sortableKeyboardCoordinates,
  verticalListSortingStrategy,
  arrayMove,
} from '@dnd-kit/sortable';
import { CSS } from '@dnd-kit/utilities';
import { restrictToVerticalAxis, restrictToParentElement } from '@dnd-kit/modifiers';
import { Logo } from './Logo';
import { UpdateNotification } from './UpdateNotification';
import { Toaster } from 'sonner';
import { getVersion } from '@tauri-apps/api/app';
import { useTranslation } from '../i18n';
import { useUpdater } from '../lib/useUpdater';
import { useTheme } from '../lib/ThemeProvider';
import type { TranslationKey } from '../i18n';

function useAppVersion() {
  const [version, setVersion] = useState('');
  useEffect(() => { getVersion().then(setVersion).catch(() => {}); }, []);
  return version;
}

const STORAGE_KEY = 'sidebar-collapsed';
const NAV_ORDER_KEY = 'sidebar-nav-order';
const LAST_ROUTE_KEY = 'last-route';
const INSTANT_TRANSITION = { duration: 0 };

type NavItem = { to: string; labelKey: TranslationKey; icon: typeof Search };

const CANONICAL_NAV_ITEMS: NavItem[] = [
  { to: '/', labelKey: 'nav.search', icon: Search },
  { to: '/sources', labelKey: 'nav.sources', icon: FolderOpen },
  { to: '/playbooks', labelKey: 'nav.playbooks', icon: BookOpen },
  { to: '/knowledge', labelKey: 'nav.knowledge', icon: Brain },
  { to: '/chat', labelKey: 'nav.chat', icon: MessageCircle },
  { to: '/settings', labelKey: 'nav.settings', icon: Settings },
];

function loadOrderedNavItems(): NavItem[] {
  try {
    const raw = localStorage.getItem(NAV_ORDER_KEY);
    if (!raw) return CANONICAL_NAV_ITEMS;
    const saved = JSON.parse(raw);
    if (!Array.isArray(saved)) return CANONICAL_NAV_ITEMS;
    const byRoute = new Map(CANONICAL_NAV_ITEMS.map((it) => [it.to, it]));
    const seen = new Set<string>();
    const ordered: NavItem[] = [];
    for (const to of saved) {
      if (typeof to !== 'string') continue;
      const item = byRoute.get(to);
      if (item && !seen.has(to)) {
        ordered.push(item);
        seen.add(to);
      }
    }
    // Append any canonical items not in saved order (forward-compat).
    for (const item of CANONICAL_NAV_ITEMS) {
      if (!seen.has(item.to)) ordered.push(item);
    }
    return ordered;
  } catch {
    return CANONICAL_NAV_ITEMS;
  }
}

/* ── Right-side tooltip for collapsed sidebar ─────────────────────── */
function SidebarTooltip({ content, show, children }: { content: string; show: boolean; children: ReactNode }) {
  const [hovered, setHovered] = useState(false);
  const shouldReduceMotion = useReducedMotion();

  if (!show) return <>{children}</>;

  return (
    <div
      className="relative"
      onMouseEnter={() => setHovered(true)}
      onMouseLeave={() => setHovered(false)}
    >
      {children}
      <AnimatePresence>
        {hovered && (
          <motion.div
            initial={shouldReduceMotion ? false : { opacity: 0, x: -4 }}
            animate={{ opacity: 1, x: 0 }}
            exit={shouldReduceMotion ? { opacity: 0, x: 0 } : { opacity: 0, x: -4 }}
            transition={shouldReduceMotion ? INSTANT_TRANSITION : { duration: 0.15 }}
            className="absolute left-full top-1/2 -translate-y-1/2 ml-2 z-50
              px-2.5 py-1.5 text-xs font-medium
              bg-surface-4 text-text-primary rounded-md shadow-md
              whitespace-nowrap pointer-events-none"
          >
            {content}
          </motion.div>
        )}
      </AnimatePresence>
    </div>
  );
}

/* ── Sortable nav item ────────────────────────────────────────────── */
interface SortableNavItemProps {
  item: NavItem;
  label: string;
  collapsed: boolean;
  isCurrentPage: boolean;
  shouldReduceMotion: boolean;
}

function SortableNavItem({ item, label, collapsed, isCurrentPage, shouldReduceMotion }: SortableNavItemProps) {
  const Icon = item.icon;
  const {
    attributes,
    listeners,
    setNodeRef,
    transform,
    transition,
    isDragging,
  } = useSortable({ id: item.to });

  const style: CSSProperties = {
    transform: CSS.Transform.toString(transform),
    transition,
    opacity: isDragging ? 0.5 : 1,
    zIndex: isDragging ? 10 : undefined,
  };

  return (
    <div ref={setNodeRef} style={style} {...attributes} {...listeners}>
      <SidebarTooltip content={label} show={collapsed}>
        <NavLink
          to={item.to}
          end={item.to === '/'}
          aria-label={label}
          aria-current={isCurrentPage ? 'page' : undefined}
          className={({ isActive }: { isActive: boolean }) =>
            `relative flex items-center gap-2.5 rounded-md text-sm transition-colors duration-fast ease-out
            ${collapsed ? 'justify-center px-0 py-2' : 'px-3 py-2'}
            ${isActive
              ? 'bg-accent-subtle text-accent-hover'
              : 'text-text-secondary hover:bg-surface-2 hover:text-text-primary'
            }`
          }
        >
          {({ isActive }: { isActive: boolean }) => (
            <>
              {/* Active indicator bar */}
              <motion.span
                className="absolute left-0 top-1/2 -translate-y-1/2 w-0.75 rounded-r-full bg-accent"
                initial={false}
                animate={{ height: isActive ? 20 : 0, opacity: isActive ? 1 : 0 }}
                transition={shouldReduceMotion ? INSTANT_TRANSITION : { duration: 0.25, ease: [0.16, 1, 0.3, 1] }}
              />
              <Icon className="h-4.5 w-4.5 shrink-0" />
              <AnimatePresence>
                {!collapsed && (
                  <motion.span
                    initial={shouldReduceMotion ? false : { opacity: 0, width: 0 }}
                    animate={{ opacity: 1, width: 'auto' }}
                    exit={{ opacity: 0, width: 0 }}
                    transition={shouldReduceMotion ? INSTANT_TRANSITION : { duration: 0.2 }}
                    className="overflow-hidden whitespace-nowrap"
                  >
                    {label}
                  </motion.span>
                )}
              </AnimatePresence>
            </>
          )}
        </NavLink>
      </SidebarTooltip>
    </div>
  );
}

/* ── Layout ───────────────────────────────────────────────────────── */
export function Layout() {
  const { t } = useTranslation();
  const { theme } = useTheme();
  const appVersion = useAppVersion();
  const updater = useUpdater();
  const updateStatus = updater.status;
  const shouldReduceMotion = useReducedMotion();
  const location = useLocation();
  const navigate = useNavigate();
  const [collapsed, setCollapsed] = useState(() => {
    try {
      return localStorage.getItem(STORAGE_KEY) === 'true';
    } catch {
      return false;
    }
  });
  const [navItems, setNavItems] = useState<NavItem[]>(() => loadOrderedNavItems());

  const sensors = useSensors(
    useSensor(PointerSensor, { activationConstraint: { distance: 6 } }),
    useSensor(KeyboardSensor, { coordinateGetter: sortableKeyboardCoordinates }),
  );

  const navIds = useMemo(() => navItems.map((it) => it.to), [navItems]);

  const handleDragEnd = (event: DragEndEvent) => {
    const { active, over } = event;
    if (!over || active.id === over.id) return;
    setNavItems((prev) => {
      const oldIndex = prev.findIndex((it) => it.to === active.id);
      const newIndex = prev.findIndex((it) => it.to === over.id);
      if (oldIndex < 0 || newIndex < 0) return prev;
      const next = arrayMove(prev, oldIndex, newIndex);
      try {
        localStorage.setItem(NAV_ORDER_KEY, JSON.stringify(next.map((it) => it.to)));
      } catch { /* noop */ }
      return next;
    });
  };

  // Persist the last visited route on every location change.
  useEffect(() => {
    try {
      localStorage.setItem(LAST_ROUTE_KEY, location.pathname);
    } catch { /* noop */ }
  }, [location.pathname]);

  // On initial mount, if we landed on the default '/' route but the user
  // was somewhere else last session, restore that route.
  const didInitialRedirect = useRef(false);
  useEffect(() => {
    if (didInitialRedirect.current) return;
    didInitialRedirect.current = true;
    try {
      const saved = localStorage.getItem(LAST_ROUTE_KEY);
      if (saved && saved !== '/' && location.pathname === '/') {
        navigate(saved, { replace: true });
      }
    } catch { /* noop */ }
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  const toggle = () => {
    setCollapsed((prev: boolean) => {
      const next = !prev;
      try { localStorage.setItem(STORAGE_KEY, String(next)); } catch { /* noop */ }
      return next;
    });
  };

  return (
    <div className="flex h-screen bg-surface-0 text-text-primary">
      {/* Sidebar */}
      <motion.aside
        className="flex shrink-0 flex-col border-r border-border bg-surface-1 overflow-hidden"
        animate={{ width: collapsed ? 56 : 208 }}
        transition={shouldReduceMotion ? INSTANT_TRANSITION : { duration: 0.25, ease: [0.16, 1, 0.3, 1] }}
        aria-label={t('nav.mainNav')}
      >
        {/* Branding */}
        <div className="flex items-center gap-2.5 px-3.5 py-4 overflow-hidden">
          <Logo size={20} className="shrink-0" />
          <AnimatePresence>
            {!collapsed && (
              <motion.div
                initial={shouldReduceMotion ? false : { opacity: 0, width: 0 }}
                animate={{ opacity: 1, width: 'auto' }}
                exit={{ opacity: 0, width: 0 }}
                transition={shouldReduceMotion ? INSTANT_TRANSITION : { duration: 0.2 }}
                className="overflow-hidden whitespace-nowrap"
              >
                <h1 className="text-sm font-bold tracking-tight text-text-primary">{t('app.name')}</h1>
              </motion.div>
            )}
          </AnimatePresence>
        </div>

        {/* Navigation */}
        <nav className="flex-1 space-y-0.5 px-2" role="navigation">
          <DndContext
            sensors={sensors}
            collisionDetection={closestCenter}
            modifiers={[restrictToVerticalAxis, restrictToParentElement]}
            onDragEnd={handleDragEnd}
          >
            <SortableContext items={navIds} strategy={verticalListSortingStrategy}>
              {navItems.map((item) => {
                const label = t(item.labelKey);
                const isCurrentPage = item.to === '/'
                  ? location.pathname === item.to
                  : location.pathname === item.to || location.pathname.startsWith(`${item.to}/`);
                return (
                  <SortableNavItem
                    key={item.to}
                    item={item}
                    label={label}
                    collapsed={collapsed}
                    isCurrentPage={isCurrentPage}
                    shouldReduceMotion={!!shouldReduceMotion}
                  />
                );
              })}
            </SortableContext>
          </DndContext>
        </nav>

        {/* Footer: collapse toggle + version */}
        <div className="border-t border-border px-2 py-2">
          <button
            onClick={toggle}
            aria-label={collapsed ? t('nav.expand') : t('nav.collapse')}
            className={`flex w-full items-center gap-2 rounded-md px-2 py-1.5 text-xs
              text-text-tertiary hover:text-text-secondary hover:bg-surface-2
              transition-colors duration-fast ease-out cursor-pointer
              ${collapsed ? 'justify-center' : ''}`}
          >
            {collapsed ? (
              <ChevronRight className="h-4 w-4 shrink-0" />
            ) : (
              <>
                <ChevronLeft className="h-4 w-4 shrink-0" />
                <span className="overflow-hidden whitespace-nowrap">{t('nav.collapse')}</span>
                <span className="ml-auto text-text-tertiary/60 relative">
                  v{appVersion}
                  {(updateStatus === 'available' || updateStatus === 'downloading' || updateStatus === 'ready') && (
                    <span className="absolute -top-0.5 -right-2 h-2 w-2 rounded-full bg-danger animate-pulse" />
                  )}
                </span>
              </>
            )}
          </button>
          {!collapsed && (
            <div className="mt-1 px-2 py-0.5 text-[11px] text-text-tertiary/60 select-none">
              {/Mac/i.test(navigator.userAgent) ? '⌘' : 'Ctrl+'}K {t('nav.commandPalette')}
            </div>
          )}
        </div>
      </motion.aside>

      {/* Main content */}
      <main className="flex-1 min-h-0 overflow-y-auto">
        <UpdateNotification updater={updater} />
        <Outlet />
      </main>

      {/* Floating AI button */}
      {!location.pathname.startsWith('/chat') && (
        <motion.button
          onClick={() => navigate('/chat')}
          aria-label={t('chat.askAi')}
          className="fixed bottom-6 right-6 z-40 p-3 rounded-full
            bg-accent text-white shadow-lg
            hover:bg-accent-hover transition-colors duration-200 cursor-pointer"
          whileHover={shouldReduceMotion ? undefined : { scale: 1.1 }}
          whileTap={shouldReduceMotion ? undefined : { scale: 0.95 }}
          title={t('chat.askAi')}
        >
          <BotMessageSquare size={22} />
        </motion.button>
      )}

      {/* Toast notifications */}
      <Toaster theme={theme === 'light' ? 'light' : 'dark'} richColors position="bottom-right" />
    </div>
  );
}
