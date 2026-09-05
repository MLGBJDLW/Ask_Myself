import { useState, useEffect, type ComponentType, type ReactNode } from "react";
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
  useRouteError,
} from "react-router";
import { listen } from "@tauri-apps/api/event";
import { motion, MotionConfig, useReducedMotion } from "framer-motion";
import { I18nProvider, useTranslation } from "./i18n";
import { ErrorBoundary, ErrorScreen } from "./components/ErrorBoundary";
import { Layout } from "./components/Layout";
import { AppWindowFrame } from "./components/AppWindowFrame";

import { CommandPalette } from "./components/CommandPalette";
import { StreamProvider } from "./lib/StreamProvider";
import { ProgressProvider } from "./lib/ProgressProvider";
import { FilePreviewProvider } from "./features/preview";
import { BrowserDock } from "./features/browser";
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

function PageLoader() {
  const { t } = useTranslation();
  return (
    <div
      className="startup-splash"
      data-testid="startup-splash"
      role="status"
      aria-label={t('common.loading')}
    >
      <div className="startup-splash__content">
        <div className="startup-splash__mark" aria-hidden="true">
          <img className="startup-splash__logo" src="/logo-small.svg" alt="" />
        </div>
        <span className="startup-splash__wordmark">Nexa</span>
      </div>
    </div>
  );
}

function withPageTransition(Component: ComponentType) {
  return function Page() {
    return <PageTransition><Component /></PageTransition>;
  };
}

function InitialRouteLoader() {
  const location = useLocation();
  return (
    <I18nProvider>
      <AppWindowFrame area={location.pathname === '/' ? 'home' : 'task'}><PageLoader /></AppWindowFrame>
    </I18nProvider>
  );
}

function RouteErrorScreen() {
  const error = useRouteError();
  return <ErrorScreen error={error instanceof Error ? error : new Error(String(error))} />;
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
        <AppWindowFrame area={location.pathname === '/' ? 'home' : 'task'}>
          <FilePreviewProvider>
            <CommandPalette />
            {wizardCompleted === false && location.pathname !== '/wizard' && (
              <Navigate to="/wizard" replace />
            )}
            <div
              data-testid="app-workspace"
              className="flex h-full min-h-0 min-w-0 overflow-hidden"
            >
              <div className="h-full min-h-0 min-w-0 flex-1">
                {wizardCompleted === null ? (
                  <PageLoader />
                ) : (
                  <Outlet context={{ setWizardCompleted } satisfies AppShellOutletContext} />
                )}
              </div>
              <GlobalBrowserDock />
            </div>
          </FilePreviewProvider>
        </AppWindowFrame>
      </MotionConfig>
    </I18nProvider>
  );
}

function GlobalBrowserDock() {
  const location = useLocation();
  const [open, setOpen] = useState(false);
  if (location.pathname.startsWith('/chat')) return null;
  return (
    <BrowserDock
      open={open}
      conversationId="nexa-global-browser-workspace"
      onOpenChange={setOpen}
    />
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
        ErrorBoundary={RouteErrorScreen}
        hydrateFallbackElement={null}
        lazy={async () => {
          const { CompanionWindowPage } = await import("./pages/CompanionWindowPage");
          return { Component: () => (
            <I18nProvider>
              <MotionConfig reducedMotion="user"><CompanionWindowPage /></MotionConfig>
            </I18nProvider>
          ) };
        }}
      />
      <Route element={<AppShell />} HydrateFallback={InitialRouteLoader} ErrorBoundary={RouteErrorScreen}>
        <Route path="/wizard" lazy={async () => ({ Component: (await import("./pages/WizardPage")).WizardPage })} />
        <Route element={<Layout />}>
          <Route path="/" lazy={async () => ({ Component: withPageTransition((await import("./pages/SearchPage")).SearchPage) })} />
          <Route path="/sources" lazy={async () => ({ Component: withPageTransition((await import("./pages/SourcesPage")).SourcesPage) })} />
          <Route path="/playbooks" element={<Navigate to="/" replace />} />
          <Route path="/knowledge" lazy={async () => ({ Component: withPageTransition((await import("./pages/KnowledgePage")).KnowledgePage) })} />
          <Route path="/chat/:conversationId?" lazy={async () => ({ Component: withPageTransition((await import("./pages/ChatPage")).ChatPage) })} />
          <Route path="/tasks" lazy={async () => ({ Component: withPageTransition((await import("./pages/TaskCenterPage")).TaskCenterPage) })} />
          <Route path="/workflows" lazy={async () => ({ Component: withPageTransition((await import("./pages/WorkflowsPage")).WorkflowsPage) })} />
          <Route path="/settings" lazy={async () => ({ Component: withPageTransition((await import("./pages/SettingsPage")).SettingsPage) })} />
          <Route path="*" element={<PageTransition><NotFoundPage /></PageTransition>} />
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
