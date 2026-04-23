import { useState, useEffect, type ReactNode } from "react";
import {
  createBrowserRouter,
  createRoutesFromElements,
  Link,
  Navigate,
  Outlet,
  Route,
  RouterProvider,
  useLocation,
} from "react-router-dom";
import { motion, MotionConfig, useReducedMotion } from "framer-motion";
import { I18nProvider, useTranslation } from "./i18n";
import { ErrorBoundary } from "./components/ErrorBoundary";
import { Layout } from "./components/Layout";

import { SearchPage } from "./pages/SearchPage";
import { SourcesPage } from "./pages/SourcesPage";
import { PlaybooksPage } from "./pages/PlaybooksPage";
import { KnowledgePage } from "./pages/KnowledgePage";
import { SettingsPage } from "./pages/SettingsPage";
import { ChatPage } from './pages/ChatPage';
import { WizardPage } from "./pages/WizardPage";
import { CommandPalette } from "./components/CommandPalette";
import { StreamProvider } from "./lib/StreamProvider";
import { ProgressProvider } from "./lib/ProgressProvider";
import * as api from "./lib/api";
import { useAutoCompile } from "./lib/useAutoCompile";
import { useAutoHealthCheck } from "./lib/useAutoHealthCheck";
import { useKnowledgeInsights } from "./lib/useKnowledgeInsights";

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
  // `null` = still loading; `true`/`false` = known state.
  const [wizardCompleted, setWizardCompleted] = useState<boolean | null>(null);
  const location = useLocation();

  useAutoCompile();
  useAutoHealthCheck();
  useKnowledgeInsights();

  useEffect(() => {
    api.getWizardState()
      .then(state => setWizardCompleted(Boolean(state?.completed)))
      .catch(() => setWizardCompleted(true)); // Fail-open: don't block on I/O errors.
  }, []);

  // Re-check after the user returns from /wizard so that subsequent navigation
  // doesn't loop back.  Cheap because the state is cached in sqlite.
  useEffect(() => {
    if (location.pathname === '/wizard') return;
    if (wizardCompleted === false) {
      api.getWizardState()
        .then(state => setWizardCompleted(Boolean(state?.completed)))
        .catch(() => {});
    }
  }, [location.pathname, wizardCompleted]);

  return (
    <I18nProvider>
      <MotionConfig reducedMotion="user">
        <CommandPalette />
        {wizardCompleted === false && location.pathname !== '/wizard' && (
          <Navigate to="/wizard" replace />
        )}
        {wizardCompleted !== null && <Outlet />}
      </MotionConfig>
    </I18nProvider>
  );
}

const router = createBrowserRouter(
  createRoutesFromElements(
    <Route element={<AppShell />}>
      <Route path="/wizard" element={<WizardPage />} />
      <Route element={<Layout />}>
        <Route path="/" element={<PageTransition><SearchPage /></PageTransition>} />
        <Route path="/sources" element={<PageTransition><SourcesPage /></PageTransition>} />
        <Route path="/playbooks" element={<PageTransition><PlaybooksPage /></PageTransition>} />
        <Route path="/knowledge" element={<PageTransition><KnowledgePage /></PageTransition>} />
        <Route path="/chat/:conversationId?" element={<PageTransition><ChatPage /></PageTransition>} />
        <Route path="/settings" element={<PageTransition><SettingsPage /></PageTransition>} />
        <Route path="*" element={<PageTransition><NotFoundPage /></PageTransition>} />
      </Route>
    </Route>
  ),
);

function App() {
  return (
    <ErrorBoundary>
      <ProgressProvider />
      <StreamProvider>
        <RouterProvider router={router} />
      </StreamProvider>
    </ErrorBoundary>
  );
}

export default App;