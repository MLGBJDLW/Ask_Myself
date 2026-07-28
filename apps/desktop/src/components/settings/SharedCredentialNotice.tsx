import { KeyRound, ShieldCheck } from 'lucide-react';

import { useTranslation } from '../../i18n';

export interface DetectedCredential {
  name: string;
  apiKey: string;
}

interface SharedCredentialNoticeProps {
  /** Credential discovered on an already configured provider that shares this endpoint scope. */
  source: DetectedCredential | null;
  /** Whether the capability currently holds its own explicitly typed key. */
  hasOwnKey: boolean;
  /** Copy the detected key into the capability field so it can be inspected or edited. */
  onApply: () => void;
  /** Clear the capability field so the detected key is reused again. */
  onReset: () => void;
  className?: string;
}

/**
 * Surfaces a credential that another configured provider already stores for the
 * same endpoint scope. The detected key is applied on save automatically, so the
 * notice explains what will happen and keeps manual entry one click away.
 */
export function SharedCredentialNotice({
  source,
  hasOwnKey,
  onApply,
  onReset,
  className = '',
}: SharedCredentialNoticeProps) {
  const { t } = useTranslation();
  if (!source) return null;

  const reusing = !hasOwnKey;
  const tone = reusing
    ? 'border-success/25 bg-success/5 text-success'
    : 'border-border/60 bg-surface-1/60 text-text-tertiary';

  return (
    <div
      data-testid="shared-credential-notice"
      data-state={reusing ? 'reusing' : 'available'}
      className={`flex flex-wrap items-start gap-2 rounded-md border p-2.5 ${tone} ${className}`}
    >
      <span className="mt-0.5 shrink-0">
        {reusing ? <ShieldCheck size={14} /> : <KeyRound size={14} />}
      </span>
      <div className="min-w-0 flex-1 space-y-1">
        <p className="text-xs font-medium leading-5">
          {reusing
            ? t('settings.sharedCredentialDetected', { provider: source.name })
            : t('settings.sharedCredentialAvailable', { provider: source.name })}
        </p>
        <p className="text-[11px] leading-5 text-text-tertiary">
          {reusing ? t('settings.sharedCredentialDetectedDesc') : t('settings.sharedCredentialAvailableDesc')}
        </p>
        <div className="flex flex-wrap gap-3 pt-0.5">
          {reusing ? (
            <button
              type="button"
              onClick={onApply}
              className="cursor-pointer text-[11px] font-medium text-accent underline-offset-2 hover:underline"
            >
              {t('settings.sharedCredentialFill')}
            </button>
          ) : (
            <>
              <button
                type="button"
                onClick={onApply}
                className="cursor-pointer text-[11px] font-medium text-accent underline-offset-2 hover:underline"
              >
                {t('settings.sharedCredentialApply')}
              </button>
              <button
                type="button"
                onClick={onReset}
                className="cursor-pointer text-[11px] font-medium text-text-tertiary underline-offset-2 hover:text-text-secondary hover:underline"
              >
                {t('settings.sharedCredentialClear')}
              </button>
            </>
          )}
        </div>
      </div>
    </div>
  );
}
