import { Component, type ErrorInfo, type ReactNode } from 'react';
import { AlertTriangle, ChevronDown, ChevronUp, RefreshCw } from 'lucide-react';

interface Props {
  children: ReactNode;
}

interface State {
  hasError: boolean;
  error: Error | null;
  showDetails: boolean;
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
    return localStorage.getItem('app-locale') || 'en';
  } catch {
    return 'en';
  }
}

const LABELS: Record<string, { title: string; description: string; restart: string; details: string }> = {
  en: {
    title: 'Something went wrong',
    description: 'An unexpected error occurred. You can try restarting the application.',
    restart: 'Restart',
    details: 'Error details',
  },
  'zh-CN': {
    title: '出现了问题',
    description: '发生了意外错误。您可以尝试重启应用。',
    restart: '重启',
    details: '错误详情',
  },
  'zh-TW': {
    title: '出現了問題',
    description: '發生了意外錯誤。您可以嘗試重新啟動應用程式。',
    restart: '重新啟動',
    details: '錯誤詳情',
  },
  ja: {
    title: '問題が発生しました',
    description: '予期しないエラーが発生しました。アプリを再起動してみてください。',
    restart: '再起動',
    details: 'エラー詳細',
  },
  ko: {
    title: '문제가 발생했습니다',
    description: '예기치 않은 오류가 발생했습니다. 앱을 다시 시작해 보세요.',
    restart: '다시 시작',
    details: '오류 상세',
  },
  fr: {
    title: 'Une erreur est survenue',
    description: 'Une erreur inattendue s\'est produite. Vous pouvez essayer de redémarrer l\'application.',
    restart: 'Redémarrer',
    details: 'Détails de l\'erreur',
  },
  de: {
    title: 'Ein Fehler ist aufgetreten',
    description: 'Ein unerwarteter Fehler ist aufgetreten. Versuchen Sie, die Anwendung neu zu starten.',
    restart: 'Neu starten',
    details: 'Fehlerdetails',
  },
  es: {
    title: 'Algo salió mal',
    description: 'Ocurrió un error inesperado. Puede intentar reiniciar la aplicación.',
    restart: 'Reiniciar',
    details: 'Detalles del error',
  },
  pt: {
    title: 'Algo deu errado',
    description: 'Ocorreu um erro inesperado. Você pode tentar reiniciar o aplicativo.',
    restart: 'Reiniciar',
    details: 'Detalhes do erro',
  },
  ru: {
    title: 'Что-то пошло не так',
    description: 'Произошла непредвиденная ошибка. Попробуйте перезапустить приложение.',
    restart: 'Перезапустить',
    details: 'Подробности ошибки',
  },
};

function getLabels() {
  const locale = getStoredLocale();
  return LABELS[locale] ?? LABELS.en;
}

export class ErrorBoundary extends Component<Props, State> {
  constructor(props: Props) {
    super(props);
    this.state = { hasError: false, error: null, showDetails: false };
  }

  static getDerivedStateFromError(error: Error): Partial<State> {
    return { hasError: true, error };
  }

  componentDidCatch(error: Error, info: ErrorInfo) {
    console.error('[ErrorBoundary]', error, info.componentStack);
  }

  handleRestart = () => {
    window.location.reload();
  };

  toggleDetails = () => {
    this.setState((prev) => ({ showDetails: !prev.showDetails }));
  };

  render() {
    if (!this.state.hasError) {
      return this.props.children;
    }

    const labels = getLabels();
    const { error, showDetails } = this.state;

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
            onClick={this.handleRestart}
            className="inline-flex items-center gap-2 rounded-lg bg-accent px-5 py-2.5 text-sm font-medium text-white transition-colors hover:bg-accent/90"
          >
            <RefreshCw size={14} />
            {labels.restart}
          </button>

          {error && (
            <div className="mt-6">
              <button
                onClick={this.toggleDetails}
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
}
