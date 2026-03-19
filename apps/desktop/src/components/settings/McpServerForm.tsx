import { useEffect, useRef, useState } from 'react';
import {
  AlertTriangle,
  CheckCircle,
  Info,
  Loader2,
  Save,
  Wand2,
  X,
  Zap,
} from 'lucide-react';
import { toast } from 'sonner';
import { Button } from '../ui/Button';
import { Input } from '../ui/Input';
import { Badge } from '../ui/Badge';
import { useTranslation } from '../../i18n';
import * as api from '../../lib/api';
import type { McpServer, McpToolInfo, SaveMcpServerInput } from '../../types/extensions';

interface McpServerFormProps {
  server?: McpServer;
  onSave: (input: SaveMcpServerInput) => void;
  onCancel: () => void;
  onDirtyChange?: (dirty: boolean) => void;
}

type TransportType = 'stdio' | 'sse' | 'streamable_http';

function parseArgsDraft(
  raw: string,
  messages: {
    invalidJson: string;
    invalidArray: string;
  },
): { values: string[]; error: string | null } {
  const trimmed = raw.trim();
  if (!trimmed) {
    return { values: [], error: null };
  }

  if (trimmed.startsWith('[')) {
    try {
      const parsed = JSON.parse(trimmed);
      if (!Array.isArray(parsed) || !parsed.every((value) => typeof value === 'string')) {
        return { values: [], error: messages.invalidArray };
      }
      return { values: parsed, error: null };
    } catch {
      return { values: [], error: messages.invalidJson };
    }
  }

  const values = (
    trimmed.includes('\n')
      ? trimmed.split('\n')
      : trimmed.includes(',')
        ? trimmed.split(',')
        : trimmed.split(/\s+/)
  )
    .map((value) => value.trim())
    .filter(Boolean);

  return { values, error: null };
}

function parseJsonObjectDraft(
  raw: string,
  label: string,
  messages: {
    mustBeObject: string;
    invalidEntries: string;
    invalidJson: string;
  },
): { normalized: string | null; error: string | null } {
  const trimmed = raw.trim();
  if (!trimmed) {
    return { normalized: null, error: null };
  }

  try {
    const parsed = JSON.parse(trimmed);
    if (parsed === null || Array.isArray(parsed) || typeof parsed !== 'object') {
      return { normalized: null, error: `${label}${messages.mustBeObject}` };
    }

    const entries = Object.entries(parsed as Record<string, unknown>);
    if (!entries.every(([key, value]) => key.trim().length > 0 && typeof value === 'string')) {
      return { normalized: null, error: `${label}${messages.invalidEntries}` };
    }

    return { normalized: JSON.stringify(parsed), error: null };
  } catch {
    return { normalized: null, error: `${label}${messages.invalidJson}` };
  }
}

function normalizeHttpUrl(raw: string): string {
  const value = raw.trim();
  if (!value) {
    return '';
  }

  const parsed = new URL(value);
  if (!['http:', 'https:'].includes(parsed.protocol)) {
    throw new Error('protocol');
  }
  return parsed.toString();
}

export function McpServerForm({ server, onSave, onCancel, onDirtyChange }: McpServerFormProps) {
  const { t } = useTranslation();

  const initialTransport = (server?.transport ?? 'stdio') as TransportType;
  const [name, setName] = useState(server?.name ?? '');
  const [transport, setTransport] = useState<TransportType>(
    ['stdio', 'sse', 'streamable_http'].includes(initialTransport) ? initialTransport : 'stdio',
  );
  const [command, setCommand] = useState(server?.command ?? '');
  const [args, setArgs] = useState(server?.args ?? '');
  const [envJson, setEnvJson] = useState(server?.envJson ?? '');
  const [url, setUrl] = useState(server?.url ?? '');
  const [headersJson, setHeadersJson] = useState(server?.headersJson ?? '');
  const [testLoading, setTestLoading] = useState(false);
  const [discoveredTools, setDiscoveredTools] = useState<McpToolInfo[] | null>(null);
  const [testError, setTestError] = useState<string | null>(null);
  const initialDraftRef = useRef({
    name: server?.name ?? '',
    transport: (server?.transport ?? 'stdio') as TransportType,
    command: server?.command ?? '',
    args: server?.args ?? '',
    envJson: server?.envJson ?? '',
    url: server?.url ?? '',
    headersJson: server?.headersJson ?? '',
  });

  const isStdio = transport === 'stdio';
  const isRemote = !isStdio;

  const parsedArgs = parseArgsDraft(args, {
    invalidJson: t('settings.mcpArgsInvalidJson'),
    invalidArray: t('settings.mcpArgsInvalidArray'),
  });
  const parsedEnv = parseJsonObjectDraft(envJson, t('settings.mcpEnvVars'), {
    mustBeObject: t('settings.mcpJsonMustBeObject'),
    invalidEntries: t('settings.mcpJsonInvalidEntries'),
    invalidJson: t('settings.mcpJsonInvalid'),
  });
  const parsedHeaders = parseJsonObjectDraft(headersJson, t('settings.mcpHeadersLabel'), {
    mustBeObject: t('settings.mcpJsonMustBeObject'),
    invalidEntries: t('settings.mcpJsonInvalidEntries'),
    invalidJson: t('settings.mcpJsonInvalid'),
  });

  const normalizedName = name.trim();
  const normalizedCommand = command.trim();
  const normalizedUrl = url.trim();

  let normalizedRemoteUrl: string | null = null;
  let urlError: string | null = null;
  if (isRemote) {
    if (!normalizedUrl) {
      urlError = t('settings.mcpUrlRequired');
    } else {
      try {
        normalizedRemoteUrl = normalizeHttpUrl(normalizedUrl);
      } catch {
        urlError = t('settings.mcpUrlInvalid');
      }
    }
  }

  const nameError = normalizedName ? null : t('settings.mcpNameRequired');
  const commandError = isStdio && !normalizedCommand ? t('settings.mcpCommandRequired') : null;
  const validationErrors = [
    nameError,
    commandError,
    isStdio ? parsedArgs.error : null,
    isStdio ? parsedEnv.error : null,
    isRemote ? urlError : null,
    isRemote ? parsedHeaders.error : null,
  ].filter(Boolean);
  const hasValidationError = validationErrors.length > 0;

  const envPortState =
    isStdio && parsedEnv.normalized
      ? Object.keys(JSON.parse(parsedEnv.normalized) as Record<string, string>).some(
          (key) => key.toUpperCase() === 'PORT',
        )
        ? ('explicit' as const)
        : ('managed' as const)
      : ('managed' as const);

  const transportCards: Array<{
    value: TransportType;
    title: string;
    hint: string;
    badge?: string;
  }> = [
    { value: 'stdio', title: t('settings.mcpStdioTitle'), hint: t('settings.mcpStdioHint') },
    {
      value: 'streamable_http',
      title: t('settings.mcpStreamableTitle'),
      hint: t('settings.mcpStreamableHint'),
      badge: t('settings.mcpRecommended'),
    },
    { value: 'sse', title: t('settings.mcpSseTitle'), hint: t('settings.mcpSseHint') },
  ];

  const buildInput = (): SaveMcpServerInput | null => {
    if (hasValidationError) {
      return null;
    }

    if (isStdio) {
      return {
        id: server?.id ?? null,
        name: normalizedName,
        transport,
        command: normalizedCommand,
        args: parsedArgs.values.length > 0 ? JSON.stringify(parsedArgs.values) : null,
        url: null,
        envJson: parsedEnv.normalized,
        headersJson: null,
        enabled: server?.enabled ?? true,
      };
    }

    return {
      id: server?.id ?? null,
      name: normalizedName,
      transport,
      command: null,
      args: null,
      url: normalizedRemoteUrl,
      envJson: null,
      headersJson: parsedHeaders.normalized,
      enabled: server?.enabled ?? true,
    };
  };

  useEffect(() => {
    if (!onDirtyChange) return;

    const dirty = (
      name !== initialDraftRef.current.name
      || transport !== initialDraftRef.current.transport
      || command !== initialDraftRef.current.command
      || args !== initialDraftRef.current.args
      || envJson !== initialDraftRef.current.envJson
      || url !== initialDraftRef.current.url
      || headersJson !== initialDraftRef.current.headersJson
    );

    onDirtyChange(dirty);
  }, [args, command, envJson, headersJson, name, onDirtyChange, transport, url]);

  useEffect(() => {
    if (!onDirtyChange) return;

    return () => {
      onDirtyChange(false);
    };
  }, [onDirtyChange]);

  const handleSubmit = () => {
    const input = buildInput();
    if (!input) {
      return;
    }
    onSave(input);
  };

  const handleTest = async () => {
    const input = buildInput();
    if (!input) {
      return;
    }

    setTestLoading(true);
    setTestError(null);
    setDiscoveredTools(null);

    try {
      const tools = await api.testMcpServerDirect({
        name: input.name,
        transport: input.transport,
        command: input.command ?? null,
        args: input.args ?? null,
        url: input.url ?? null,
        envJson: input.envJson ?? null,
        headersJson: input.headersJson ?? null,
      });
      setDiscoveredTools(tools);
      toast.success(t('settings.mcpTestSuccess', { count: String(tools.length) }));
    } catch (error: unknown) {
      const message = error instanceof Error ? error.message : String(error);
      setTestError(message);
      toast.error(`${t('settings.mcpTestFailed')}: ${message}`);
    } finally {
      setTestLoading(false);
    }
  };

  return (
    <div className="space-y-4">
      <div className="rounded-xl border border-border bg-surface-2 p-4 space-y-3">
        <div className="flex items-start gap-2">
          <Info size={16} className="mt-0.5 shrink-0 text-accent" />
          <div className="space-y-1">
            <p className="text-sm font-medium text-text-primary">{t('settings.mcpTransport')}</p>
            <p className="text-xs text-text-tertiary">{t('settings.mcpTransportHelp')}</p>
          </div>
        </div>
        <div className="grid gap-2 md:grid-cols-3">
          {transportCards.map((item) => {
            const active = transport === item.value;
            return (
              <button
                key={item.value}
                type="button"
                disabled={!!server?.builtinId}
                onClick={() => setTransport(item.value)}
                className={`rounded-xl border p-3 text-left transition-colors disabled:opacity-50 disabled:cursor-not-allowed ${
                  active
                    ? 'border-accent bg-accent/10'
                    : 'border-border bg-surface-1 hover:bg-surface-2'
                }`}
              >
                <div className="flex items-center gap-2">
                  <span className="text-sm font-medium text-text-primary">{item.title}</span>
                  {item.badge && (
                    <Badge variant="default" className="text-[10px] bg-accent/10 text-accent border-accent/20">
                      {item.badge}
                    </Badge>
                  )}
                </div>
                <p className="mt-1 text-xs text-text-tertiary">{item.hint}</p>
              </button>
            );
          })}
        </div>
      </div>

      <div className="space-y-2">
        <label className="text-sm font-medium text-text-primary">{t('settings.mcpServerName')}</label>
        <Input
          value={name}
          onChange={(event) => setName(event.target.value)}
          placeholder={t('settings.mcpServerName')}
          error={nameError ?? undefined}
          disabled={!!server?.builtinId}
        />
      </div>

      {isStdio ? (
        <>
          <div className="space-y-2">
            <label className="text-sm font-medium text-text-primary">{t('settings.mcpCommand')}</label>
            <Input
              value={command}
              onChange={(event) => setCommand(event.target.value)}
              placeholder="npx"
              error={commandError ?? undefined}
              disabled={!!server?.builtinId}
            />
          </div>

          <div className="space-y-2">
            <label className="text-sm font-medium text-text-primary">{t('settings.mcpArgs')}</label>
            <textarea
              value={args}
              onChange={(event) => setArgs(event.target.value)}
              placeholder='["-y", "@modelcontextprotocol/server-filesystem", "D:/vault"]'
              rows={4}
              disabled={!!server?.builtinId}
              className={`w-full rounded-md border bg-surface-2 px-3 py-2 text-sm font-mono text-text-primary placeholder:text-text-tertiary focus:outline-none focus:ring-1 resize-y disabled:opacity-50 disabled:cursor-not-allowed ${
                parsedArgs.error
                  ? 'border-danger focus:border-danger focus:ring-danger/30'
                  : 'border-border focus:border-accent focus:ring-accent'
              }`}
            />
            <p className="text-xs text-text-tertiary">{t('settings.mcpArgsHelp')}</p>
            {parsedArgs.error && <p className="text-xs text-danger">{parsedArgs.error}</p>}
            {parsedArgs.values.length > 0 && (
              <div className="rounded-lg border border-border bg-surface-2 p-3 space-y-2">
                <p className="text-[11px] uppercase tracking-wide text-text-tertiary">{t('settings.mcpParsedArgs')}</p>
                <div className="flex flex-wrap gap-1.5">
                  {parsedArgs.values.map((value, index) => (
                    <Badge key={`${value}-${index}`} variant="default" className="max-w-full text-[10px]">
                      {value}
                    </Badge>
                  ))}
                </div>
              </div>
            )}
          </div>

          <div className="space-y-2">
            <label className="text-sm font-medium text-text-primary">{t('settings.mcpEnvVars')}</label>
            <textarea
              value={envJson}
              onChange={(event) => setEnvJson(event.target.value)}
              placeholder='{"API_KEY": "..."}'
              rows={4}
              className={`w-full rounded-md border bg-surface-2 px-3 py-2 text-sm font-mono text-text-primary placeholder:text-text-tertiary focus:outline-none focus:ring-1 resize-y ${
                parsedEnv.error
                  ? 'border-danger focus:border-danger focus:ring-danger/30'
                  : 'border-border focus:border-accent focus:ring-accent'
              }`}
            />
            <p className="text-xs text-text-tertiary">{t('settings.mcpEnvHelp')}</p>
            {parsedEnv.error && <p className="text-xs text-danger">{parsedEnv.error}</p>}
          </div>
        </>
      ) : (
        <>
          <div className="space-y-2">
            <label className="text-sm font-medium text-text-primary">{t('settings.mcpUrlLabel')}</label>
            <Input
              value={url}
              onChange={(event) => setUrl(event.target.value)}
              placeholder={transport === 'sse' ? 'https://example.com/sse' : 'https://example.com/mcp'}
              error={urlError ?? undefined}
              disabled={!!server?.builtinId}
            />
            <p className="text-xs text-text-tertiary">{t('settings.mcpUrlHelp')}</p>
          </div>

          <div className="space-y-2">
            <label className="text-sm font-medium text-text-primary">{t('settings.mcpHeadersLabel')}</label>
            <textarea
              value={headersJson}
              onChange={(event) => setHeadersJson(event.target.value)}
              placeholder='{"Authorization": "Bearer ..."}'
              rows={4}
              disabled={!!server?.builtinId}
              className={`w-full rounded-md border bg-surface-2 px-3 py-2 text-sm font-mono text-text-primary placeholder:text-text-tertiary focus:outline-none focus:ring-1 resize-y disabled:opacity-50 disabled:cursor-not-allowed ${
                parsedHeaders.error
                  ? 'border-danger focus:border-danger focus:ring-danger/30'
                  : 'border-border focus:border-accent focus:ring-accent'
              }`}
            />
            <p className="text-xs text-text-tertiary">{t('settings.mcpHeadersHelp')}</p>
            {parsedHeaders.error && <p className="text-xs text-danger">{parsedHeaders.error}</p>}
          </div>
        </>
      )}

      <div
        className={`rounded-xl border p-4 ${
          hasValidationError ? 'border-warning/30 bg-warning/10' : 'border-success/20 bg-success/10'
        }`}
      >
        <div className="flex items-start gap-2">
          {hasValidationError ? (
            <AlertTriangle size={16} className="mt-0.5 shrink-0 text-warning" />
          ) : (
            <CheckCircle size={16} className="mt-0.5 shrink-0 text-success" />
          )}
          <div className="space-y-1">
            <p className={`text-sm font-medium ${hasValidationError ? 'text-warning' : 'text-success'}`}>
              {hasValidationError ? t('settings.mcpDraftInvalid') : t('settings.mcpDraftReady')}
            </p>
            <p className="text-xs text-text-tertiary">
              {isStdio
                ? envPortState === 'managed'
                  ? t('settings.mcpFreePortManaged')
                  : t('settings.mcpExplicitPort')
                : t('settings.mcpRemoteSessionNote')}
            </p>
          </div>
        </div>
      </div>

      <div className="space-y-2">
        <Button
          variant="secondary"
          size="sm"
          icon={testLoading ? <Loader2 size={14} className="animate-spin" /> : <Wand2 size={14} />}
          loading={testLoading}
          onClick={handleTest}
          disabled={hasValidationError}
        >
          {t('settings.mcpTestConnection')}
        </Button>
        {testError && (
          <div className="rounded-lg border border-danger/30 bg-danger/5 p-3">
            <p className="text-sm text-danger whitespace-pre-wrap break-all">{testError}</p>
          </div>
        )}
        {discoveredTools && (
          <div className="rounded-lg border border-border bg-surface-2 p-3 space-y-2">
            <div className="flex items-center gap-2">
              <Zap size={14} className="text-accent" />
              <p className="text-sm font-medium text-text-primary">
                {t('settings.mcpDiscoveredTools')} ({discoveredTools.length})
              </p>
            </div>
            <div className="flex flex-wrap gap-1.5">
              {discoveredTools.map((tool) => (
                <Badge key={tool.name} variant="default" className="text-[10px]">
                  {tool.name}
                </Badge>
              ))}
            </div>
          </div>
        )}
      </div>

      <div className="flex items-center gap-2 pt-2 border-t border-border">
        <Button
          variant="primary"
          size="sm"
          icon={<Save size={14} />}
          onClick={handleSubmit}
          disabled={hasValidationError}
        >
          {t('common.save')}
        </Button>
        <Button variant="ghost" size="sm" icon={<X size={14} />} onClick={onCancel}>
          {t('common.cancel')}
        </Button>
      </div>
    </div>
  );
}
