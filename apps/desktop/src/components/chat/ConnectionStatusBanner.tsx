import { AlertTriangle, CheckCircle2, RotateCw, WifiOff } from 'lucide-react';
import { useEffect, useState } from 'react';
import { useTranslation } from '../../i18n';

import type { ProviderConnectionState } from '../../types/conversation';

interface ConnectionStatusBannerProps {
  connection: ProviderConnectionState | null;
}

export function ConnectionStatusBanner({ connection }: ConnectionStatusBannerProps) {
  const { t } = useTranslation();
  const [showRecovered, setShowRecovered] = useState(false);

  useEffect(() => {
    if (connection?.state !== 'recovered') {
      setShowRecovered(false);
      return undefined;
    }
    setShowRecovered(true);
    const timer = window.setTimeout(() => setShowRecovered(false), 3_000);
    return () => window.clearTimeout(timer);
  }, [connection]);

  if (!connection || (connection.state === 'recovered' && !showRecovered)) return null;

  const failed = connection.state === 'failed' || connection.state === 'offline';
  const recovered = connection.state === 'recovered';
  const label = connection.state === 'reconnecting'
    ? t('chat.connectionReconnecting', { defaultValue: 'Reconnecting to the provider' })
    : connection.state === 'degraded'
      ? t('chat.connectionDegraded', { defaultValue: 'Provider connection is degraded' })
      : connection.state === 'recovered'
        ? t('chat.connectionRecovered', { defaultValue: 'Provider connection recovered' })
        : connection.state === 'offline'
          ? t('chat.connectionOffline', { defaultValue: 'Provider is offline' })
          : t('chat.connectionFailed', { defaultValue: 'Provider connection failed' });
  const Icon = recovered ? CheckCircle2 : failed ? WifiOff : connection.state === 'reconnecting'
    ? RotateCw
    : AlertTriangle;
  const detail = connection.turnPreserved
    ? t('chat.connectionTurnPreserved', { defaultValue: 'Your current turn is preserved.' })
    : t('chat.connectionTurnNotPreserved', { defaultValue: 'This turn could not be preserved.' });
  const nextAction = failed
    ? t('chat.connectionNextAction', {
        defaultValue: 'Check provider settings, then resend when the connection is available.',
      })
    : null;

  return (
    <div className="shrink-0 px-4 pb-2" role={failed ? 'alert' : 'status'} aria-live="polite">
      <div className={`mx-auto flex max-w-4xl items-center gap-2 rounded-lg border px-3 py-2 text-xs ${
        failed
          ? 'border-danger/30 bg-danger/10 text-danger'
          : recovered
            ? 'border-success/30 bg-success/10 text-success'
            : 'border-warning/30 bg-warning/10 text-text-secondary'
      }`}>
        <Icon
          className={`h-3.5 w-3.5 shrink-0 ${connection.state === 'reconnecting' ? 'animate-spin motion-reduce:animate-none' : ''}`}
        />
        <div className="min-w-0 flex-1">
          <div className="font-medium text-text-primary">{label}</div>
          <div className="truncate text-[11px] opacity-80">
            {connection.providerId} · {connection.modelId}
            {connection.maxAttempts > 0 && connection.state === 'reconnecting'
              ? ` · ${connection.attempt}/${connection.maxAttempts}`
              : ''}
            {` · ${detail}`}
            {nextAction ? ` ${nextAction}` : ''}
          </div>
        </div>
      </div>
    </div>
  );
}
