import { type ReactNode } from "react";
import {
  createBrowserRouter,
  createRoutesFromElements,
  Link,
  Outlet,
  Route,
  RouterProvider,
} from "react-router-dom";
import { motion, MotionConfig, useReducedMotion } from "framer-motion";
import { I18nProvider, useTranslation } from "./i18n";
import { Layout } from "./components/Layout";
import { SearchPage } from "./pages/SearchPage";
import { SourcesPage } from "./pages/SourcesPage";
import { PlaybooksPage } from "./pages/PlaybooksPage";
import { SettingsPage } from "./pages/SettingsPage";
import { ChatPage } from './pages/ChatPage';
import { CommandPalette } from "./components/CommandPalette";

/* ── Page transition wrapper ─────────────────────────────────────── */
function PageTransition({ children }: { children: ReactNode }) {
  const shouldReduceMotion = useReducedMotion();

  if (shouldReduceMotion) {
    return <div className="h-full min-h-0">{children}</div>;
  }

  return (
    <motion.div
      initial={{ opacity: 0, y: 6 }}
      animate={{ opacity: 1, y: 0 }}
      transition={{ duration: 0.2, ease: [0.16, 1, 0.3, 1] }}
      className="h-full min-h-0"
    >
      {children}
    </motion.div>
  );
}

function NotFoundPage() {
  const { t } = useTranslation();

  return (
    <div className="flex-1 flex flex-col items-center justify-center gap-4">
      <p className="text-4xl font-bold text-text-primary">404</p>
      <p className="text-sm text-text-tertiary">{t('app.pageNotFound')}</p>
      <Link to="/" className="px-4 py-2 rounded-lg bg-accent text-white text-sm hover:bg-accent/90 transition-colors">
        {t('app.goHome')}
      </Link>
    </div>
  );
}

function AppShell() {
  return (
    <I18nProvider>
      <MotionConfig reducedMotion="user">
        <CommandPalette />
        <Outlet />
      </MotionConfig>
    </I18nProvider>
  );
}

const router = createBrowserRouter(
  createRoutesFromElements(
    <Route element={<AppShell />}>
      <Route element={<Layout />}>
        <Route path="/" element={<PageTransition><SearchPage /></PageTransition>} />
        <Route path="/sources" element={<PageTransition><SourcesPage /></PageTransition>} />
        <Route path="/playbooks" element={<PageTransition><PlaybooksPage /></PageTransition>} />
        <Route path="/chat" element={<PageTransition><ChatPage /></PageTransition>} />
        <Route path="/chat/:conversationId" element={<PageTransition><ChatPage /></PageTransition>} />
        <Route path="/settings" element={<PageTransition><SettingsPage /></PageTransition>} />
        <Route path="*" element={<PageTransition><NotFoundPage /></PageTransition>} />
      </Route>
    </Route>
  ),
);

function App() {
  return <RouterProvider router={router} />;
}

export default App;