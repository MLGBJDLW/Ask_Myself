import { Suspense, lazy, useState, useEffect, type ReactNode } from "react";
import {
  createBrowserRouter,
  createRoutesFromElements,
  Link,
  Navigate,
  Outlet,
  Route,
  RouterProvider,
  useLocation,
  useNavigate,
} from "react-router";
import { listen } from "@tauri-apps/api/event";
import { motion, MotionConfig, useReducedMotion } from "framer-motion";
import { I18nProvider, useTranslation } from "./i18n";
import { ErrorBoundary } from "./components/ErrorBoundary";
import { Layout } from "./components/Layout";
import { AppWindowFrame } from "./components/AppWindowFrame";

import { CommandPalette } from "./components/CommandPalette";
import { StreamProvider } from "./lib/StreamProvider";
import { ProgressProvider } from "./lib/ProgressProvider";
import { FilePreviewProvider } from "./features/preview";
import * as api from "./lib/api";
import { useAutoCompile } from "./lib/useAutoCompile";
import { useAutoHealthCheck } from "./lib/useAutoHealthCheck";
import { useKnowledgeInsights } from "./lib/useKnowledgeInsights";

const SearchPage = lazy(() => import("./pages/SearchPage").then((module) => ({ default: module.SearchPage })));
const SourcesPage = lazy(() => import("./pages/SourcesPage").then((module) => ({ default: module.SourcesPage })));
const KnowledgePage = lazy(() => import("./pages/KnowledgePage").then((module) => ({ default: module.KnowledgePage })));
const SettingsPage = lazy(() => import("./pages/SettingsPage").then((module) => ({ default: module.SettingsPage })));
const ChatPage = lazy(() => import("./pages/ChatPage").then((module) => ({ default: module.ChatPage })));
const TaskCenterPage = lazy(() => import("./pages/TaskCenterPage").then((module) => ({ default: module.TaskCenterPage })));
const WorkflowsPage = lazy(() => import("./pages/WorkflowsPage").then((module) => ({ default: module.WorkflowsPage })));
const WizardPage = lazy(() => import("./pages/WizardPage").then((module) => ({ default: module.WizardPage })));
const CompanionWindowPage = lazy(() => import("./pages/CompanionWindowPage").then((module) => ({ default: module.CompanionWindowPage })));

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

function PageLoader() {
  const { t } = useTranslation();
  return (
    <div className="flex h-full min-h-0 items-center justify-center text-sm text-text-tertiary">
      {t('common.loading')}
    </div>
  );
}

function LazyPage({ children }: { children: ReactNode }) {
  return (
    <Suspense fallback={<PageLoader />}>
      <PageTransition>{children}</PageTransition>
    </Suspense>
  );
}

function AppShell() {
  // `null` = still loading; `true`/`false` = known state.
  const [wizardCompleted, setWizardCompleted] = useState<boolean | null>(null);
  const location = useLocation();
  const navigate = useNavigate();

  useAutoCompile();
  useAutoHealthCheck(wizardCompleted === true);
  useKnowledgeInsights(wizardCompleted === true);

  useEffect(() => {
    api.getWizardState()
      .then(state => setWizardCompleted(state == null ? true : Boolean(state.completed)))
      .catch(() => setWizardCompleted(true)); // Fail-open: don't block on I/O errors.
  }, []);

  // Re-check whenever we think the wizard is incomplete.  This is a
  // belt-and-braces backstop: in practice WizardPage pushes the new state via
  // outlet context on success, so this refetch is a no-op.  Idempotent —
  // once `wizardCompleted === true`, the guard in the effect short-circuits
  // and no loop is possible.
  useEffect(() => {
    if (wizardCompleted === false) {
      api.getWizardState()
        .then(state => setWizardCompleted(state == null ? true : Boolean(state.completed)))
        .catch(() => {});
    }
  }, [location.pathname, wizardCompleted]);

  useEffect(() => {
    const unlisten = listen('companion://open-settings', () => navigate('/settings'));
    return () => { void unlisten.then((callback) => callback()); };
  }, [navigate]);

  return (
    <I18nProvider>
      <MotionConfig reducedMotion="user">
        <AppWindowFrame>
          <FilePreviewProvider>
            <CommandPalette />
            {wizardCompleted === false && location.pathname !== '/wizard' && (
              <Navigate to="/wizard" replace />
            )}
            {wizardCompleted === null ? (
              <PageLoader />
            ) : (
              <Outlet context={{ setWizardCompleted } satisfies AppShellOutletContext} />
            )}
          </FilePreviewProvider>
        </AppWindowFrame>
      </MotionConfig>
    </I18nProvider>
  );
}

/** Outlet context exposed by {@link AppShell} to nested routes (e.g. WizardPage). */
export type AppShellOutletContext = {
  setWizardCompleted: (completed: boolean) => void;
};

const router = createBrowserRouter(
  createRoutesFromElements(
    <>
      <Route
        path="/companion"
        element={(
          <I18nProvider>
            <MotionConfig reducedMotion="user">
              <Suspense fallback={null}><CompanionWindowPage /></Suspense>
            </MotionConfig>
          </I18nProvider>
        )}
      />
      <Route element={<AppShell />}>
        <Route path="/wizard" element={<Suspense fallback={<PageLoader />}><WizardPage /></Suspense>} />
        <Route element={<Layout />}>
          <Route path="/" element={<LazyPage><SearchPage /></LazyPage>} />
          <Route path="/sources" element={<LazyPage><SourcesPage /></LazyPage>} />
          <Route path="/playbooks" element={<Navigate to="/" replace />} />
          <Route path="/knowledge" element={<LazyPage><KnowledgePage /></LazyPage>} />
          <Route path="/chat/:conversationId?" element={<LazyPage><ChatPage /></LazyPage>} />
          <Route path="/tasks" element={<LazyPage><TaskCenterPage /></LazyPage>} />
          <Route path="/workflows" element={<LazyPage><WorkflowsPage /></LazyPage>} />
          <Route path="/settings" element={<LazyPage><SettingsPage /></LazyPage>} />
          <Route path="*" element={<LazyPage><NotFoundPage /></LazyPage>} />
        </Route>
      </Route>
    </>
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
