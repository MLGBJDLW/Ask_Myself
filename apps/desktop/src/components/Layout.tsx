import { useState, useEffect, useMemo, useRef, type CSSProperties } from 'react';
import { NavLink, Outlet, useLocation, useNavigate } from 'react-router';
import { motion, useReducedMotion } from 'framer-motion';
import { Search, FolderOpen, MessageCircle, Settings, Brain, BotMessageSquare, ClipboardList, Workflow, Download, Loader2, CheckCircle2, AlertCircle, RefreshCw } from 'lucide-react';
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
import { Tooltip } from './ui';
import { Toaster } from 'sonner';
import { getVersion } from '@tauri-apps/api/app';
import { useTranslation } from '../i18n';
import { useUpdater } from '../lib/useUpdater';
import { useTheme } from '../lib/ThemeProvider';
import { isLightTheme } from '../lib/theme';
import type { TranslationKey } from '../i18n';

function useAppVersion() {
  const [version, setVersion] = useState('');
  useEffect(() => { getVersion().then(setVersion).catch(() => {}); }, []);
  return version;
}

const NAV_ORDER_KEY = 'sidebar-nav-order';
const LAST_ROUTE_KEY = 'last-route';
const INSTANT_TRANSITION = { duration: 0 };
const WALLPAPER_ADAPTED_PAGE_ROUTES = ['/settings', '/tasks', '/workflows'] as const;

type NavItem = { to: string; labelKey: TranslationKey; icon: typeof Search };

const CANONICAL_NAV_ITEMS: NavItem[] = [
  { to: '/', labelKey: 'nav.search', icon: Search },
  { to: '/sources', labelKey: 'nav.sources', icon: FolderOpen },
  { to: '/knowledge', labelKey: 'nav.knowledge', icon: Brain },
  { to: '/chat', labelKey: 'nav.chat', icon: MessageCircle },
  { to: '/tasks', labelKey: 'nav.tasks', icon: ClipboardList },
  { to: '/workflows', labelKey: 'nav.workflows', icon: Workflow },
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

function isWallpaperAdaptedPageRoute(pathname: string): boolean {
  return WALLPAPER_ADAPTED_PAGE_ROUTES.some((route) => (
    pathname === route || pathname.startsWith(`${route}/`)
  ));
}

/* ── Sortable nav item ────────────────────────────────────────────── */
interface SortableNavItemProps {
  item: NavItem;
  label: string;
  isCurrentPage: boolean;
  shouldReduceMotion: boolean;
}

function SortableNavItem({ item, label, isCurrentPage, shouldReduceMotion }: SortableNavItemProps) {
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
    <div ref={setNodeRef} style={style} {...attributes} {...listeners} className="mx-auto w-10">
      <Tooltip content={label} side="right" delay={180}>
        <NavLink
          to={item.to}
          end={item.to === '/'}
          aria-label={label}
          aria-current={isCurrentPage ? 'page' : undefined}
          className={({ isActive }: { isActive: boolean }) =>
            `relative flex h-10 w-10 items-center justify-center rounded-md text-sm transition-colors duration-fast ease-out
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
                className="absolute -left-2 top-1/2 w-0.75 -translate-y-1/2 rounded-r-full bg-accent"
                initial={false}
                animate={{ height: isActive ? 20 : 0, opacity: isActive ? 1 : 0 }}
                transition={shouldReduceMotion ? INSTANT_TRANSITION : { duration: 0.25, ease: [0.16, 1, 0.3, 1] }}
              />
              <Icon className="h-4.5 w-4.5 shrink-0" />
            </>
          )}
        </NavLink>
      </Tooltip>
    </div>
  );
}

/* ── Layout ───────────────────────────────────────────────────────── */
export function Layout() {
  const { t } = useTranslation();
  const { theme } = useTheme();
  const appVersion = useAppVersion();
  const updater = useUpdater(true);
  const shouldReduceMotion = useReducedMotion();
  const location = useLocation();
  const navigate = useNavigate();
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

  const updateLabel = updater.status === 'available'
    ? t('update.version', { version: updater.version ?? '' })
    : updater.status === 'downloading'
      ? t('update.downloading')
      : updater.status === 'ready'
        ? t('update.ready')
        : updater.status === 'error'
          ? t('update.error')
          : updater.status === 'up-to-date'
            ? t('update.upToDate')
            : t('update.checkNow');
  const UpdateIcon = updater.status === 'available'
    ? Download
    : updater.status === 'downloading' || updater.status === 'checking'
      ? Loader2
      : updater.status === 'ready' || updater.status === 'up-to-date'
        ? CheckCircle2
        : updater.status === 'error'
          ? AlertCircle
          : RefreshCw;
  const mainThemeSurface = location.pathname.startsWith('/chat')
    ? 'transparent'
    : isWallpaperAdaptedPageRoute(location.pathname)
      ? 'page'
      : 'content';

  return (
    <div
      className="relative flex h-full min-h-0 overflow-hidden bg-surface-0 text-text-primary"
      data-theme-surface="workspace"
    >
      {/* Sidebar */}
      <aside
        className="relative z-10 flex w-14 shrink-0 flex-col overflow-hidden border-r border-border bg-surface-1"
        aria-label={t('nav.mainNav')}
        data-testid="app-navigation-rail"
      >
        {/* Branding */}
        <div className="grid h-14 shrink-0 place-items-center">
          <Tooltip content={t('app.name')} side="right" delay={180}>
            <NavLink to="/" aria-label={t('app.name')} className="grid h-10 w-10 place-items-center rounded-md hover:bg-surface-2">
              <Logo size={22} decorative />
            </NavLink>
          </Tooltip>
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
                    isCurrentPage={isCurrentPage}
                    shouldReduceMotion={!!shouldReduceMotion}
                  />
                );
              })}
            </SortableContext>
          </DndContext>
        </nav>

        {/* Stable metadata and update controls */}
        <div className="space-y-1 border-t border-border px-2 py-2" data-theme-density-part="rail-footer">
          <Tooltip content={t('nav.settings')} side="right" delay={180}>
            <NavLink
              to="/settings"
              aria-label={t('nav.settings')}
              className={({ isActive }) => `grid h-10 w-10 place-items-center rounded-md transition-colors ${isActive ? 'bg-accent-subtle text-accent-hover' : 'text-text-tertiary hover:bg-surface-2 hover:text-text-primary'}`}
            >
              <Settings className="h-4.5 w-4.5" />
            </NavLink>
          </Tooltip>
          <Tooltip content={updateLabel} side="right" delay={180}>
            <button
              type="button"
              onClick={() => navigate('/settings')}
              aria-label={updateLabel}
              data-update-status={updater.status}
              className={`relative grid h-10 w-10 place-items-center rounded-md transition-colors hover:bg-surface-2 ${updater.status === 'error' ? 'text-danger' : updater.status === 'available' ? 'text-warning' : updater.status === 'ready' ? 'text-success' : 'text-text-tertiary hover:text-text-primary'}`}
            >
              <UpdateIcon className={`h-4.5 w-4.5 ${updater.status === 'checking' || updater.status === 'downloading' ? 'animate-spin' : ''}`} />
              {(updater.status === 'available' || updater.status === 'ready') && (
                <span className="absolute right-1.5 top-1.5 h-1.5 w-1.5 rounded-full bg-current shadow-[0_0_7px_currentColor]" />
              )}
            </button>
          </Tooltip>
          <Tooltip content={`${t('update.currentVersion')}: v${appVersion || '—'}`} side="right" delay={180}>
            <div className="flex h-5 w-10 select-none items-center justify-center overflow-hidden text-[9px] font-medium tracking-tight text-text-tertiary/65" data-testid="app-version">
              {appVersion ? `v${appVersion}` : 'v—'}
            </div>
          </Tooltip>
        </div>
      </aside>

      {/* Main content */}
      <main
        className="relative z-10 flex-1 min-w-0 min-h-0 overflow-y-auto"
        data-theme-surface={mainThemeSurface}
      >
        <Outlet />
      </main>

      {/* Floating AI button */}
      {!location.pathname.startsWith('/chat') && (
        <button
          onClick={() => navigate('/chat')}
          aria-label={t('chat.askAi')}
          className="fixed bottom-4 right-4 z-40 p-2.5 rounded-full sm:bottom-6 sm:right-6 sm:p-3
            bg-accent text-white shadow-lg
            hover:bg-accent-hover transition-colors duration-200 cursor-pointer"
          title={t('chat.askAi')}
        >
          <BotMessageSquare size={22} />
        </button>
      )}

      {/* Toast notifications */}
      <Toaster theme={isLightTheme(theme) ? 'light' : 'dark'} richColors position="bottom-right" />
    </div>
  );
}
