import { useCallback, useEffect, useMemo, useState } from 'react';
import { open } from '@tauri-apps/plugin-shell';
import {
  Check,
  CircleAlert,
  Copy,
  ExternalLink,
  Github,
  Loader2,
  LogIn,
  LogOut,
  RefreshCw,
  ShieldCheck,
  TerminalSquare,
  X,
} from 'lucide-react';
import { toast } from 'sonner';
import { useTranslation } from '../../i18n';
import * as api from '../../lib/api';
import type {
  CodexAccountSnapshot,
  CodexLoginDescriptor,
  CodexRateLimitWindow,
  CopilotAccountSnapshot,
} from '../../lib/api';
import { Badge } from '../ui/Badge';
import { Button } from '../ui/Button';

const CODEX_DOCS_URL = 'https://developers.openai.com/codex/app-server';
const COPILOT_DOCS_URL = 'https://docs.github.com/en/copilot/how-tos/copilot-sdk/auth/authenticate';
const LOGIN_POLL_MS = 1_500;
const COPILOT_LOGIN_POLL_MS = 3_000;

function useSerialPoll(enabled: boolean, poll: () => Promise<void>, delayMs: number): void {
  useEffect(() => {
    if (!enabled) return undefined;
    let cancelled = false;
    let timer: number | undefined;
    const schedule = () => {
      timer = window.setTimeout(() => {
        void poll().finally(() => {
          if (!cancelled) schedule();
        });
      }, delayMs);
    };
    schedule();
    return () => {
      cancelled = true;
      if (timer != null) window.clearTimeout(timer);
    };
  }, [delayMs, enabled, poll]);
}

function planLabel(plan: string | null | undefined): string | null {
  if (!plan) return null;
  return plan
    .split('_')
    .map((part) => part ? `${part[0].toUpperCase()}${part.slice(1)}` : part)
    .join(' ');
}

function formatReset(timestamp: number | null, locale: string): string | null {
  if (!timestamp) return null;
  return new Intl.DateTimeFormat(locale, {
    month: 'short',
    day: 'numeric',
    hour: '2-digit',
    minute: '2-digit',
  }).format(new Date(timestamp * 1_000));
}

function UsageWindow({
  label,
  window,
  locale,
}: {
  label: string;
  window: CodexRateLimitWindow;
  locale: string;
}) {
  const { t } = useTranslation();
  const used = Math.max(0, Math.min(100, window.usedPercent));
  const reset = formatReset(window.resetsAt, locale);
  return (
    <div className="space-y-1.5">
      <div className="flex items-center justify-between gap-3 text-[11px] text-text-tertiary">
        <span>{label}</span>
        <span>{t('settings.subscriptionRemaining', { percent: 100 - used })}</span>
      </div>
      <div className="h-1.5 overflow-hidden rounded-full bg-surface-4" role="meter" aria-valuemin={0} aria-valuemax={100} aria-valuenow={100 - used}>
        <div
          className="h-full rounded-full bg-accent transition-[width] duration-normal"
          style={{ width: `${100 - used}%` }}
        />
      </div>
      {reset && (
        <p className="text-[10px] text-text-tertiary">
          {t('settings.subscriptionResets', { date: reset })}
        </p>
      )}
    </div>
  );
}

function runtimeErrorCopy(errorCode: string | null, t: ReturnType<typeof useTranslation>['t']): string {
  if (errorCode === 'codex_runtime_not_found') return t('settings.codexRuntimeMissing');
  if (errorCode === 'codex_runtime_invalid_override') return t('settings.codexRuntimeOverrideInvalid');
  if (errorCode === 'codex_runtime_request_timeout') return t('settings.codexRuntimeTimeout');
  return t('settings.codexRuntimeUnavailable');
}

function copilotRuntimeErrorCopy(errorCode: string | null, t: ReturnType<typeof useTranslation>['t']): string {
  if (errorCode === 'copilot_runtime_invalid_override') return t('settings.copilotRuntimeOverrideInvalid');
  if (errorCode === 'copilot_runtime_timeout') return t('settings.copilotRuntimeTimeout');
  if (errorCode === 'copilot_entitlement_unverified') return t('settings.copilotEntitlementUnverified');
  return t('settings.copilotRuntimeUnavailable');
}

async function openTrustedUrl(url: string | null): Promise<void> {
  if (!url) throw new Error('missing URL');
  const parsed = new URL(url);
  if (parsed.protocol !== 'https:') throw new Error('untrusted URL');
  await open(parsed.toString());
}

export function SubscriptionAccountsPanel() {
  const { t, locale } = useTranslation();
  const [snapshot, setSnapshot] = useState<CodexAccountSnapshot | null>(null);
  const [copilotSnapshot, setCopilotSnapshot] = useState<CopilotAccountSnapshot | null>(null);
  const [loading, setLoading] = useState(true);
  const [copilotLoading, setCopilotLoading] = useState(true);
  const [action, setAction] = useState<'refresh' | 'browser' | 'deviceCode' | 'cancel' | 'logout' | null>(null);
  const [copied, setCopied] = useState(false);
  const [copilotAction, setCopilotAction] = useState<'refresh' | 'login' | 'cancel' | null>(null);

  const refresh = useCallback(async (foreground = false) => {
    if (foreground) setAction('refresh');
    try {
      setSnapshot(await api.getCodexAccountSnapshot());
    } catch {
      toast.error(t('settings.subscriptionRefreshError'));
    } finally {
      setLoading(false);
      if (foreground) setAction(null);
    }
  }, [t]);

  useEffect(() => {
    void refresh();
  }, [refresh]);

  const refreshCopilot = useCallback(async (foreground = false) => {
    if (foreground) setCopilotAction('refresh');
    try {
      setCopilotSnapshot(await api.getCopilotAccountSnapshot());
    } catch {
      toast.error(t('settings.copilotRefreshError'));
    } finally {
      setCopilotLoading(false);
      if (foreground) setCopilotAction(null);
    }
  }, [t]);

  useEffect(() => {
    void refreshCopilot();
  }, [refreshCopilot]);

  useSerialPoll(Boolean(snapshot?.pendingLogin), refresh, LOGIN_POLL_MS);
  useSerialPoll(Boolean(copilotSnapshot?.loginPending), refreshCopilot, COPILOT_LOGIN_POLL_MS);

  useEffect(() => {
    if (!snapshot?.lastLogin || snapshot.lastLogin.success) return;
    toast.error(snapshot.lastLogin.error === 'login_expired'
      ? t('settings.subscriptionLoginExpired')
      : t('settings.subscriptionLoginError'));
  }, [snapshot?.lastLogin, t]);

  const beginLogin = useCallback(async (kind: 'browser' | 'deviceCode') => {
    setAction(kind);
    try {
      const pending = await api.startCodexAccountLogin(kind);
      setSnapshot((current) => current ? { ...current, pendingLogin: pending, lastLogin: null } : current);
      await openTrustedUrl(pending.authUrl ?? pending.verificationUrl);
    } catch {
      toast.error(t('settings.subscriptionLoginError'));
    } finally {
      setAction(null);
    }
  }, [t]);

  const cancelLogin = useCallback(async (pending: CodexLoginDescriptor) => {
    setAction('cancel');
    try {
      await api.cancelCodexAccountLogin(pending.loginId);
      setSnapshot((current) => current ? { ...current, pendingLogin: null, lastLogin: null } : current);
    } catch {
      toast.error(t('settings.subscriptionCancelError'));
    } finally {
      setAction(null);
    }
  }, [t]);

  const logout = useCallback(async () => {
    setAction('logout');
    try {
      setSnapshot(await api.logoutCodexAccount());
      toast.success(t('settings.subscriptionLoggedOut'));
    } catch {
      toast.error(t('settings.subscriptionLogoutError'));
    } finally {
      setAction(null);
    }
  }, [t]);

  const copyCode = useCallback(async (code: string) => {
    try {
      await navigator.clipboard.writeText(code);
      setCopied(true);
      window.setTimeout(() => setCopied(false), 1_500);
    } catch {
      toast.error(t('settings.subscriptionCopyError'));
    }
  }, [t]);

  const beginCopilotLogin = useCallback(async () => {
    setCopilotAction('login');
    try {
      await api.startCopilotAccountLogin();
      setCopilotSnapshot((current) => current ? { ...current, loginPending: true, loginError: null } : current);
    } catch {
      toast.error(t('settings.copilotLoginError'));
    } finally {
      setCopilotAction(null);
    }
  }, [t]);

  const cancelCopilotLogin = useCallback(async () => {
    setCopilotAction('cancel');
    try {
      await api.cancelCopilotAccountLogin();
      setCopilotSnapshot((current) => current ? { ...current, loginPending: false, loginError: null } : current);
    } catch {
      toast.error(t('settings.copilotCancelError'));
    } finally {
      setCopilotAction(null);
    }
  }, [t]);

  const account = snapshot?.account ?? null;
  const pending = snapshot?.pendingLogin ?? null;
  const rateLimits = useMemo(() => snapshot?.rateLimits.slice(0, 4) ?? [], [snapshot?.rateLimits]);

  return (
    <div className="space-y-3" data-provider-category="subscription-accounts">
      <div>
        <div className="flex items-center gap-2 text-xs font-semibold uppercase tracking-wide text-text-tertiary">
          <ShieldCheck size={14} />
          <span>{t('settings.subscriptionAccounts')}</span>
        </div>
        <p className="mt-1 text-xs leading-5 text-text-tertiary">
          {t('settings.subscriptionAccountsDesc')}
        </p>
      </div>

      <div className="rounded-lg border border-border bg-surface-2 p-4" data-testid="codex-subscription-account">
        <div className="flex flex-col gap-4 sm:flex-row sm:items-start sm:justify-between">
          <div className="flex min-w-0 items-start gap-3">
            <span className="inline-flex h-9 w-9 shrink-0 items-center justify-center rounded-lg border border-border bg-surface-3 text-text-secondary">
              <TerminalSquare size={19} />
            </span>
            <div className="min-w-0">
              <div className="flex flex-wrap items-center gap-2">
                <h3 className="text-sm font-semibold text-text-primary">Codex / ChatGPT</h3>
                {loading ? (
                  <Badge variant="muted" icon={<Loader2 size={10} className="animate-spin" />}>
                    {t('common.loading')}
                  </Badge>
                ) : account ? (
                  <Badge variant="success" icon={<Check size={10} />}>
                    {t('settings.subscriptionSignedIn')}
                  </Badge>
                ) : snapshot?.available ? (
                  <Badge variant="muted">{t('settings.subscriptionSignedOut')}</Badge>
                ) : (
                  <Badge variant="warning" icon={<CircleAlert size={10} />}>
                    {t('settings.subscriptionRuntimeMissing')}
                  </Badge>
                )}
                {planLabel(account?.planType) && (
                  <Badge variant="accent">{planLabel(account?.planType)}</Badge>
                )}
              </div>
              <p className="mt-1 break-all text-xs leading-5 text-text-tertiary">
                {account?.email ?? t('settings.codexCredentialOwner')}
              </p>
              {snapshot?.runtimeVersion && (
                <p className="mt-0.5 text-[10px] text-text-tertiary">{snapshot.runtimeVersion}</p>
              )}
            </div>
          </div>

          <div className="flex shrink-0 flex-wrap justify-end gap-2">
            <Button
              variant="ghost"
              size="sm"
              icon={<RefreshCw size={13} />}
              loading={action === 'refresh'}
              onClick={() => { void refresh(true); }}
            >
              {t('settings.subscriptionRefresh')}
            </Button>
            {account ? (
              <Button
                variant="secondary"
                size="sm"
                icon={<LogOut size={13} />}
                loading={action === 'logout'}
                onClick={() => { void logout(); }}
              >
                {t('settings.subscriptionSignOut')}
              </Button>
            ) : snapshot?.available && !pending ? (
              <>
                <Button
                  variant="primary"
                  size="sm"
                  icon={<LogIn size={13} />}
                  loading={action === 'browser'}
                  onClick={() => { void beginLogin('browser'); }}
                >
                  {t('settings.subscriptionSignIn')}
                </Button>
                <Button
                  variant="secondary"
                  size="sm"
                  loading={action === 'deviceCode'}
                  onClick={() => { void beginLogin('deviceCode'); }}
                >
                  {t('settings.subscriptionDeviceCode')}
                </Button>
              </>
            ) : null}
          </div>
        </div>

        {!loading && !snapshot?.available && (
          <div className="mt-4 rounded-md border border-warning/25 bg-warning/5 p-3">
            <p className="text-xs leading-5 text-text-secondary">
              {runtimeErrorCopy(snapshot?.errorCode ?? null, t)}
            </p>
            <button
              type="button"
              className="mt-2 inline-flex items-center gap-1.5 text-xs font-medium text-accent hover:underline"
              onClick={() => { void openTrustedUrl(CODEX_DOCS_URL); }}
            >
              {t('settings.subscriptionRuntimeDocs')} <ExternalLink size={12} />
            </button>
          </div>
        )}

        {pending && (
          <div className="mt-4 rounded-md border border-accent/25 bg-accent/5 p-3" data-testid="codex-login-pending">
            <div className="flex flex-col gap-3 sm:flex-row sm:items-center sm:justify-between">
              <div className="min-w-0">
                <p className="text-xs font-medium text-text-primary">
                  {pending.kind === 'deviceCode'
                    ? t('settings.subscriptionDevicePrompt')
                    : t('settings.subscriptionBrowserPrompt')}
                </p>
                {pending.userCode && (
                  <button
                    type="button"
                    className="mt-2 inline-flex items-center gap-2 rounded-md border border-border bg-surface-3 px-2.5 py-1.5 font-mono text-sm font-semibold tracking-widest text-text-primary"
                    onClick={() => { void copyCode(pending.userCode!); }}
                  >
                    {pending.userCode}
                    {copied ? <Check size={13} className="text-success" /> : <Copy size={13} />}
                  </button>
                )}
              </div>
              <div className="flex shrink-0 gap-2">
                <Button
                  variant="secondary"
                  size="sm"
                  icon={<ExternalLink size={13} />}
                  onClick={() => { void openTrustedUrl(pending.authUrl ?? pending.verificationUrl); }}
                >
                  {t('settings.subscriptionOpenBrowser')}
                </Button>
                <Button
                  variant="ghost"
                  size="sm"
                  icon={<X size={13} />}
                  loading={action === 'cancel'}
                  onClick={() => { void cancelLogin(pending); }}
                >
                  {t('common.cancel')}
                </Button>
              </div>
            </div>
          </div>
        )}

        {account && rateLimits.length > 0 && (
          <div className="mt-4 grid gap-3 border-t border-border/70 pt-4 md:grid-cols-2">
            {rateLimits.map((bucket) => (
              <div key={bucket.id} className="rounded-md border border-border/70 bg-surface-1/35 p-3">
                <div className="mb-2 flex items-center justify-between gap-2">
                  <p className="min-w-0 truncate text-xs font-medium text-text-secondary">
                    {bucket.name ?? bucket.id}
                  </p>
                  {planLabel(bucket.planType) && (
                    <Badge variant="muted" className="shrink-0">{planLabel(bucket.planType)}</Badge>
                  )}
                </div>
                <div className="space-y-2.5">
                  {bucket.primary && (
                    <UsageWindow label={t('settings.subscriptionPrimaryWindow')} window={bucket.primary} locale={locale} />
                  )}
                  {bucket.secondary && (
                    <UsageWindow label={t('settings.subscriptionSecondaryWindow')} window={bucket.secondary} locale={locale} />
                  )}
                </div>
              </div>
            ))}
          </div>
        )}

        {account && snapshot?.usage?.lifetimeTokens != null && (
          <p className="mt-3 text-[10px] text-text-tertiary">
            {t('settings.subscriptionLifetimeUsage', {
              tokens: new Intl.NumberFormat(locale).format(snapshot.usage.lifetimeTokens),
            })}
          </p>
        )}

        <p className="mt-3 text-[10px] leading-4 text-text-tertiary">
          {t('settings.subscriptionOwnershipNotice')}
        </p>
      </div>

      <div className="rounded-lg border border-border bg-surface-2 p-4" data-testid="copilot-subscription-account">
        <div className="flex flex-col gap-4 sm:flex-row sm:items-start sm:justify-between">
          <div className="flex min-w-0 items-start gap-3">
            <span className="inline-flex h-9 w-9 shrink-0 items-center justify-center rounded-lg border border-border bg-surface-3 text-text-secondary">
              <Github size={19} />
            </span>
            <div className="min-w-0">
              <div className="flex flex-wrap items-center gap-2">
                <h3 className="text-sm font-semibold text-text-primary">GitHub Copilot</h3>
                {copilotLoading ? (
                  <Badge variant="muted" icon={<Loader2 size={10} className="animate-spin" />}>
                    {t('common.loading')}
                  </Badge>
                ) : copilotSnapshot?.entitlementVerified ? (
                  <Badge variant="success" icon={<Check size={10} />}>
                    {t('settings.copilotSubscriptionVerified')}
                  </Badge>
                ) : copilotSnapshot?.authenticated ? (
                  <Badge variant="warning">{t('settings.copilotSignedInUnverified')}</Badge>
                ) : copilotSnapshot?.available ? (
                  <Badge variant="muted">{t('settings.subscriptionSignedOut')}</Badge>
                ) : (
                  <Badge variant="warning" icon={<CircleAlert size={10} />}>
                    {t('settings.subscriptionRuntimeMissing')}
                  </Badge>
                )}
              </div>
              <p className="mt-1 break-all text-xs leading-5 text-text-tertiary">
                {copilotSnapshot?.login ?? t('settings.copilotCredentialOwner')}
                {copilotSnapshot?.host ? ` · ${copilotSnapshot.host}` : ''}
              </p>
              {copilotSnapshot?.runtimeVersion && (
                <p className="mt-0.5 text-[10px] text-text-tertiary">
                  GitHub Copilot CLI {copilotSnapshot.runtimeVersion}
                  {copilotSnapshot.authType ? ` · ${copilotSnapshot.authType}` : ''}
                </p>
              )}
            </div>
          </div>

          <div className="flex shrink-0 flex-wrap justify-end gap-2">
            <Button
              variant="ghost"
              size="sm"
              icon={<RefreshCw size={13} />}
              loading={copilotAction === 'refresh'}
              onClick={() => { void refreshCopilot(true); }}
            >
              {t('settings.subscriptionRefresh')}
            </Button>
            {!copilotLoading
              && copilotSnapshot?.available
              && !copilotSnapshot.authenticated
              && !copilotSnapshot.loginPending && (
              <Button
                variant="primary"
                size="sm"
                icon={<LogIn size={13} />}
                loading={copilotAction === 'login'}
                onClick={() => { void beginCopilotLogin(); }}
              >
                {t('settings.copilotSignIn')}
              </Button>
            )}
            {copilotSnapshot?.loginPending && (
              <Button
                variant="secondary"
                size="sm"
                icon={<X size={13} />}
                loading={copilotAction === 'cancel'}
                onClick={() => { void cancelCopilotLogin(); }}
              >
                {t('common.cancel')}
              </Button>
            )}
          </div>
        </div>

        {copilotSnapshot?.loginPending && (
          <div className="mt-4 rounded-md border border-accent/25 bg-accent/5 p-3" data-testid="copilot-login-pending">
            <p className="text-xs font-medium text-text-primary">{t('settings.copilotBrowserPrompt')}</p>
          </div>
        )}

        {!copilotLoading && (copilotSnapshot?.errorCode || copilotSnapshot?.loginError) && (
          <div className="mt-4 rounded-md border border-warning/25 bg-warning/5 p-3">
            <p className="text-xs leading-5 text-text-secondary">
              {copilotRuntimeErrorCopy(copilotSnapshot.errorCode ?? copilotSnapshot.loginError, t)}
            </p>
            <button
              type="button"
              className="mt-2 inline-flex items-center gap-1.5 text-xs font-medium text-accent hover:underline"
              onClick={() => { void openTrustedUrl(COPILOT_DOCS_URL); }}
            >
              {t('settings.subscriptionRuntimeDocs')} <ExternalLink size={12} />
            </button>
          </div>
        )}

        {copilotSnapshot?.entitlementVerified && (
          <div className="mt-4 space-y-3 border-t border-border/70 pt-4">
            <div>
              <p className="text-xs font-medium text-text-secondary">
                {t('settings.copilotAvailableModels', { count: copilotSnapshot.models.length })}
              </p>
              <div className="mt-2 flex flex-wrap gap-1.5">
                {copilotSnapshot.models.slice(0, 8).map((model) => (
                  <Badge key={model.id} variant="muted" title={model.id}>{model.name}</Badge>
                ))}
              </div>
            </div>
            {copilotSnapshot.quotas.length > 0 && (
              <div className="grid gap-3 md:grid-cols-2">
                {copilotSnapshot.quotas.map((quota) => (
                  <div key={quota.id} className="rounded-md border border-border/70 bg-surface-1/35 p-3">
                    <div className="flex items-center justify-between gap-3 text-[11px] text-text-tertiary">
                      <span className="truncate">{quota.id}</span>
                      <span>
                        {quota.unlimited
                          ? t('settings.copilotUnlimited')
                          : t('settings.subscriptionRemaining', { percent: Math.round(quota.remainingPercent) })}
                      </span>
                    </div>
                    <div className="mt-1.5 h-1.5 overflow-hidden rounded-full bg-surface-4" role="meter" aria-valuemin={0} aria-valuemax={100} aria-valuenow={quota.unlimited ? 100 : quota.remainingPercent}>
                      <div
                        className="h-full rounded-full bg-accent transition-[width] duration-normal"
                        style={{ width: `${quota.unlimited ? 100 : quota.remainingPercent}%` }}
                      />
                    </div>
                    {quota.resetDate && (
                      <p className="mt-1 text-[10px] text-text-tertiary">
                        {t('settings.subscriptionResets', { date: quota.resetDate })}
                      </p>
                    )}
                  </div>
                ))}
              </div>
            )}
          </div>
        )}

        <p className="mt-3 text-[10px] leading-4 text-text-tertiary">
          {t('settings.copilotOwnershipNotice')}
        </p>
      </div>
    </div>
  );
}
