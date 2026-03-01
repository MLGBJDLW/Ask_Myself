import { useState, type ReactNode } from 'react';
import { NavLink, Outlet, useLocation, useNavigate } from 'react-router-dom';
import { motion, AnimatePresence } from 'framer-motion';
import { Search, FolderOpen, BookOpen, MessageCircle, Settings, ChevronLeft, ChevronRight, BotMessageSquare } from 'lucide-react';
import { Logo } from './Logo';
import { Toaster } from 'sonner';
import { useTranslation } from '../i18n';
import type { TranslationKey } from '../i18n';

const STORAGE_KEY = 'sidebar-collapsed';

const navItems: { to: string; labelKey: TranslationKey; icon: typeof Search }[] = [
  { to: '/', labelKey: 'nav.search', icon: Search },
  { to: '/sources', labelKey: 'nav.sources', icon: FolderOpen },
  { to: '/playbooks', labelKey: 'nav.playbooks', icon: BookOpen },
  { to: '/chat', labelKey: 'nav.chat', icon: MessageCircle },
  { to: '/settings', labelKey: 'nav.settings', icon: Settings },
];

/* ── Right-side tooltip for collapsed sidebar ─────────────────────── */
function SidebarTooltip({ content, show, children }: { content: string; show: boolean; children: ReactNode }) {
  const [hovered, setHovered] = useState(false);

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
            initial={{ opacity: 0, x: -4 }}
            animate={{ opacity: 1, x: 0 }}
            exit={{ opacity: 0, x: -4 }}
            transition={{ duration: 0.15 }}
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

/* ── Layout ───────────────────────────────────────────────────────── */
export function Layout() {
  const { t } = useTranslation();
  const location = useLocation();
  const navigate = useNavigate();
  const [collapsed, setCollapsed] = useState(() => {
    try {
      return localStorage.getItem(STORAGE_KEY) === 'true';
    } catch {
      return false;
    }
  });

  const toggle = () => {
    setCollapsed((prev) => {
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
        transition={{ duration: 0.25, ease: [0.16, 1, 0.3, 1] }}
        aria-label={t('nav.mainNav')}
      >
        {/* Branding */}
        <div className="flex items-center gap-2.5 px-3.5 py-4 overflow-hidden">
          <Logo size={20} className="shrink-0" />
          <AnimatePresence>
            {!collapsed && (
              <motion.div
                initial={{ opacity: 0, width: 0 }}
                animate={{ opacity: 1, width: 'auto' }}
                exit={{ opacity: 0, width: 0 }}
                transition={{ duration: 0.2 }}
                className="overflow-hidden whitespace-nowrap"
              >
                <h1 className="text-sm font-bold tracking-tight text-text-primary">Ask Myself</h1>
              </motion.div>
            )}
          </AnimatePresence>
        </div>

        {/* Navigation */}
        <nav className="flex-1 space-y-0.5 px-2" role="navigation">
          {navItems.map((item) => {
            const Icon = item.icon;
            const label = t(item.labelKey);
            return (
              <SidebarTooltip key={item.to} content={label} show={collapsed}>
                <NavLink
                  to={item.to}
                  end={item.to === '/'}
                  aria-label={label}
                  className={({ isActive }) =>
                    `relative flex items-center gap-2.5 rounded-md text-sm transition-colors duration-fast ease-out
                    ${collapsed ? 'justify-center px-0 py-2' : 'px-3 py-2'}
                    ${isActive
                      ? 'bg-accent-subtle text-accent-hover'
                      : 'text-text-secondary hover:bg-surface-2 hover:text-text-primary'
                    }`
                  }
                >
                  {({ isActive }) => (
                    <>
                      {/* Active indicator bar */}
                      <motion.span
                        className="absolute left-0 top-1/2 -translate-y-1/2 w-[3px] rounded-r-full bg-accent"
                        initial={false}
                        animate={{ height: isActive ? 20 : 0, opacity: isActive ? 1 : 0 }}
                        transition={{ duration: 0.25, ease: [0.16, 1, 0.3, 1] }}
                      />
                      <Icon className="h-[18px] w-[18px] shrink-0" />
                      <AnimatePresence>
                        {!collapsed && (
                          <motion.span
                            initial={{ opacity: 0, width: 0 }}
                            animate={{ opacity: 1, width: 'auto' }}
                            exit={{ opacity: 0, width: 0 }}
                            transition={{ duration: 0.2 }}
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
            );
          })}
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
                <span className="ml-auto text-text-tertiary/60">{t('app.version')}</span>
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
        <Outlet />
      </main>

      {/* Floating AI button */}
      {!location.pathname.startsWith('/chat') && (
        <motion.button
          onClick={() => navigate('/chat')}
          className="fixed bottom-6 right-6 z-40 p-3 rounded-full
            bg-accent text-white shadow-lg
            hover:bg-accent-hover hover:scale-110
            active:scale-95 transition-all duration-200 cursor-pointer"
          whileHover={{ scale: 1.1 }}
          whileTap={{ scale: 0.95 }}
          title={t('chat.askAi')}
        >
          <BotMessageSquare size={22} />
        </motion.button>
      )}

      {/* Toast notifications */}
      <Toaster theme="dark" richColors position="bottom-right" />
    </div>
  );
}
