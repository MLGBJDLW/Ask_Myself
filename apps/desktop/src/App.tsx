import { useState, useEffect, type ReactNode } from "react";
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
import { ErrorBoundary } from "./components/ErrorBoundary";
import { Layout } from "./components/Layout";

import { WelcomeWizard } from "./components/WelcomeWizard";
import { SearchPage } from "./pages/SearchPage";
import { SourcesPage } from "./pages/SourcesPage";
import { PlaybooksPage } from "./pages/PlaybooksPage";
import { KnowledgePage } from "./pages/KnowledgePage";
import { SettingsPage } from "./pages/SettingsPage";
import { ChatPage } from './pages/ChatPage';
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
  const [showWizard, setShowWizard] = useState<boolean | null>(null);

  useAutoCompile();
  useAutoHealthCheck();
  useKnowledgeInsights();

  useEffect(() => {
    api.listSources().then(sources => {
      setShowWizard(sources.length === 0);
    }).catch(() => {
      setShowWizard(false);
    });
  }, []);

  return (
    <I18nProvider>
      <MotionConfig reducedMotion="user">
        <CommandPalette />
        {showWizard && (
          <WelcomeWizard onComplete={() => setShowWizard(false)} />
        )}
        {showWizard === false && <Outlet />}
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