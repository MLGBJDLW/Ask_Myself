export interface ToolCardDiffLike {
  path?: string | null;
  absolutePath?: string | null;
}

export interface ToolCardDiffStatsLike {
  path?: string | null;
  paths?: Array<string | null | undefined> | null;
}

export type ToolCardArgsStatus = 'pending' | 'streaming' | 'ready' | 'done' | 'error';

export interface ToolCardTitleTargetInput {
  toolName?: string | null;
  renderKind?: string | null;
  args?: string;
  argsStatus?: ToolCardArgsStatus | null;
  targetOverride?: string | null;
}

function truncateMiddle(value: string, max = 42): string {
  if (value.length <= max) return value;
  const head = Math.max(8, Math.floor((max - 1) * 0.42));
  const tail = Math.max(8, max - head - 1);
  return `${value.slice(0, head)}\u2026${value.slice(-tail)}`;
}

function normalizeOneLine(value: string): string {
  return value.trim().replace(/\s+/g, ' ');
}

function parseArgsRecord(raw?: string): Record<string, unknown> | null {
  if (!raw) return null;
  try {
    const parsed = JSON.parse(raw);
    return parsed && typeof parsed === 'object' && !Array.isArray(parsed)
      ? parsed as Record<string, unknown>
      : null;
  } catch {
    return null;
  }
}

function firstStringArg(parsed: Record<string, unknown>, keys: string[]): { key: string; value: string } | null {
  for (const key of keys) {
    const value = parsed[key];
    if (typeof value === 'string' && value.trim()) {
      return { key, value: value.trim() };
    }
  }
  return null;
}

function firstArrayCountArg(parsed: Record<string, unknown>, keys: string[]): string | null {
  for (const key of keys) {
    const value = parsed[key];
    if (Array.isArray(value) && value.length > 0) {
      const noun = key.toLowerCase().includes('file') || key.toLowerCase().includes('path')
        ? 'file'
        : 'item';
      return `${value.length} ${noun}${value.length === 1 ? '' : 's'}`;
    }
  }
  return null;
}

function toolLeafName(name: string): string {
  const parts = name
    .split(/[.:/]/)
    .filter(Boolean)
    .map((part) => part.trim())
    .filter(Boolean);
  return parts.length > 0 ? parts[parts.length - 1] : name.trim();
}

export function isCommandExecutionTool(toolName?: string | null, renderKind?: string | null): boolean {
  if (renderKind === 'commandExecution') return true;
  const lower = (toolName ?? '').toLowerCase();
  const leaf = toolLeafName(lower);
  return leaf === 'run_shell'
    || leaf === 'shell_command'
    || leaf === 'run_command'
    || lower.includes('shell_command');
}

function commandArgsPreview(value: unknown): string {
  if (Array.isArray(value)) {
    return value
      .map((item) => {
        if (typeof item === 'string') return item;
        if (typeof item === 'number' || typeof item === 'boolean') return String(item);
        return '';
      })
      .filter(Boolean)
      .join(' ');
  }
  if (typeof value === 'string') return value;
  return '';
}

function getCommandBriefTarget(parsed: Record<string, unknown>): string | null {
  const command = firstStringArg(parsed, ['command', 'cmd', 'script']);
  if (command) return formatToolTarget('command', command.value);

  const program = firstStringArg(parsed, ['program', 'executable', 'bin']);
  if (!program) return null;

  const argText = commandArgsPreview(parsed.args ?? parsed.arguments ?? parsed.argv);
  const target = [program.value, argText].filter(Boolean).join(' ');
  return target ? formatToolTarget('command', target) : null;
}

export function formatToolTarget(key: string, value: string): string {
  const normalized = normalizeOneLine(value);
  const pathLikeKeys = new Set(['path', 'file', 'filename', 'filepath', 'resourcepath', 'sourcepath', 'cwd']);
  const quotedKeys = new Set(['query', 'regex', 'pattern', 'topic', 'prompt', 'description']);
  const keyLower = key.toLowerCase();
  if (pathLikeKeys.has(keyLower)) {
    return truncateMiddle(normalized.replace(/\\/g, '/'), 44);
  }
  if (quotedKeys.has(keyLower)) {
    return `"${truncateMiddle(normalized, 40)}"`;
  }
  return truncateMiddle(normalized, 44);
}

export function getToolBriefTarget(args?: string): string | null {
  const parsed = parseArgsRecord(args);
  if (!parsed) return args ? truncateMiddle(normalizeOneLine(args), 44) : null;
  const counted = firstArrayCountArg(parsed, ['paths', 'files', 'filePaths', 'resourcePaths', 'items']);
  if (counted) return counted;
  const picked = firstStringArg(parsed, [
    'path',
    'file',
    'filename',
    'filePath',
    'resourcePath',
    'sourcePath',
    'url',
    'query',
    'regex',
    'pattern',
    'topic',
    'prompt',
    'command',
    'program',
    'skillId',
    'name',
    'description',
  ]);
  return picked ? formatToolTarget(picked.key, picked.value) : null;
}

export function getToolTitleTarget({
  toolName,
  renderKind,
  args,
  argsStatus,
  targetOverride,
}: ToolCardTitleTargetInput): string | null {
  if (targetOverride != null) return targetOverride.trim() ? targetOverride : null;

  if (!isCommandExecutionTool(toolName, renderKind)) {
    return getToolBriefTarget(args);
  }

  if (argsStatus === 'pending' || argsStatus === 'streaming') {
    return null;
  }

  const parsed = parseArgsRecord(args);
  return parsed ? getCommandBriefTarget(parsed) : null;
}

export function getStableFileChangeTarget(
  diff?: ToolCardDiffLike | null,
  diffStats?: ToolCardDiffStatsLike | null,
): string | null {
  const diffPath = typeof diff?.path === 'string' && diff.path.trim()
    ? diff.path
    : typeof diff?.absolutePath === 'string' && diff.absolutePath.trim()
      ? diff.absolutePath
      : null;
  if (diffPath) return formatToolTarget('path', diffPath);

  const statsPath = typeof diffStats?.path === 'string' && diffStats.path.trim()
    ? diffStats.path
    : Array.isArray(diffStats?.paths)
      ? diffStats.paths.find((path): path is string => typeof path === 'string' && path.trim().length > 0) ?? null
      : null;
  return statsPath ? formatToolTarget('path', statsPath) : null;
}
