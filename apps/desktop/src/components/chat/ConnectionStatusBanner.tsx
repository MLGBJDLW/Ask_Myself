import { WifiOff } from 'lucide-react';
import { useTranslation } from '../../i18n';

import type { ProviderConnectionState } from '../../types/conversation';

interface ConnectionStatusBannerProps {
  connection: ProviderConnectionState | null;
}

export function ConnectionStatusBanner({ connection }: ConnectionStatusBannerProps) {
  const { t } = useTranslation();
  const failed = connection?.state === 'failed' || connection?.state === 'offline';
  if (!connection || !failed) return null;

  const label = connection.state === 'offline'
    ? t('chat.connectionOffline', { defaultValue: 'Provider is offline' })
    : t('chat.connectionFailed', { defaultValue: 'Provider connection failed' });
  const detail = connection.turnPreserved
    ? t('chat.connectionTurnPreserved', { defaultValue: 'Your current turn is preserved.' })
    : t('chat.connectionTurnNotPreserved', { defaultValue: 'This turn could not be preserved.' });
  const nextAction = t('chat.connectionNextAction', {
    defaultValue: 'Check provider settings, then resend when the connection is available.',
  });

  return (
    <div className="shrink-0 px-4 pb-2" role="alert" aria-live="polite">
      <div className="mx-auto flex max-w-4xl items-center gap-2 rounded-lg border border-danger/30 bg-danger/10 px-3 py-2 text-xs text-danger">
        <WifiOff className="h-3.5 w-3.5 shrink-0" />
        <div className="min-w-0 flex-1">
          <div className="font-medium text-text-primary">{label}</div>
          <div className="truncate text-[11px] opacity-80">
            {connection.providerId} · {connection.modelId}
            {` · ${detail}`}
            {` ${nextAction}`}
          </div>
        </div>
      </div>
    </div>
  );
}
