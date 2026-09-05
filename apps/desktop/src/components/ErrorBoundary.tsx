import { Component, useState, type ErrorInfo, type ReactNode } from 'react';
import { AlertTriangle, ChevronDown, ChevronUp, RefreshCw } from 'lucide-react';
import enErrorBoundary from '../i18n/locales/en/errorBoundary.json';
import zhCNErrorBoundary from '../i18n/locales/zh-CN/errorBoundary.json';
import zhTWErrorBoundary from '../i18n/locales/zh-TW/errorBoundary.json';
import jaErrorBoundary from '../i18n/locales/ja/errorBoundary.json';
import koErrorBoundary from '../i18n/locales/ko/errorBoundary.json';
import frErrorBoundary from '../i18n/locales/fr/errorBoundary.json';
import deErrorBoundary from '../i18n/locales/de/errorBoundary.json';
import esErrorBoundary from '../i18n/locales/es/errorBoundary.json';
import ptErrorBoundary from '../i18n/locales/pt/errorBoundary.json';
import ruErrorBoundary from '../i18n/locales/ru/errorBoundary.json';

interface Props {
  children: ReactNode;
}

interface State {
  hasError: boolean;
  error: Error | null;
}

/**
 * Top-level error boundary that catches unhandled React rendering errors
 * and shows a friendly fallback UI instead of a white screen.
 *
 * i18n note: this component renders *outside* the I18nProvider, so we
 * cannot call `useTranslation()`.  We import the current locale's
 * translations directly via a thin helper that reads `localStorage`.
 */

function getStoredLocale(): string {
  try {
    return localStorage.getItem('nexa-locale') || 'en';
  } catch {
    return 'en';
  }
}

const LABELS = {
  en: enErrorBoundary,
  'zh-CN': zhCNErrorBoundary,
  'zh-TW': zhTWErrorBoundary,
  ja: jaErrorBoundary,
  ko: koErrorBoundary,
  fr: frErrorBoundary,
  de: deErrorBoundary,
  es: esErrorBoundary,
  pt: ptErrorBoundary,
  ru: ruErrorBoundary,
};

function getLabels() {
  const locale = getStoredLocale();
  return LABELS[locale as keyof typeof LABELS] ?? LABELS.en;
}

export class ErrorBoundary extends Component<Props, State> {
  constructor(props: Props) {
    super(props);
    this.state = { hasError: false, error: null };
  }

  static getDerivedStateFromError(error: Error): Partial<State> {
    return { hasError: true, error };
  }

  componentDidCatch(error: Error, info: ErrorInfo) {
    console.error('[ErrorBoundary]', error, info.componentStack);
  }

  render() {
    return this.state.hasError
      ? <ErrorScreen error={this.state.error} />
      : this.props.children;
  }
}

export function ErrorScreen({ error }: { error: Error | null }) {
  const labels = getLabels();
  const [showDetails, setShowDetails] = useState(false);
    return (
      <div className="flex h-screen w-screen items-center justify-center bg-surface-0 p-8">
        <div className="mx-auto max-w-md text-center">
          <div className="mx-auto mb-6 flex h-16 w-16 items-center justify-center rounded-2xl bg-amber-500/10">
            <AlertTriangle size={32} className="text-amber-500" />
          </div>

          <h1 className="mb-2 text-xl font-semibold text-text-primary">
            {labels.title}
          </h1>
          <p className="mb-6 text-sm text-text-tertiary">
            {labels.description}
          </p>

          <button
            onClick={() => window.location.reload()}
            className="inline-flex items-center gap-2 rounded-lg bg-accent px-5 py-2.5 text-sm font-medium text-white transition-colors hover:bg-accent/90"
          >
            <RefreshCw size={14} />
            {labels.restart}
          </button>

          {error && (
            <div className="mt-6">
              <button
                onClick={() => setShowDetails(current => !current)}
                className="inline-flex items-center gap-1 text-xs text-text-tertiary transition-colors hover:text-text-secondary"
              >
                {showDetails ? <ChevronUp size={12} /> : <ChevronDown size={12} />}
                {labels.details}
              </button>

              {showDetails && (
                <pre className="mt-2 max-h-48 overflow-auto rounded-lg border border-border bg-surface-1 p-3 text-left text-xs text-text-secondary">
                  {error.message}
                  {error.stack && `\n\n${error.stack}`}
                </pre>
              )}
            </div>
          )}
        </div>
      </div>
    );
}
