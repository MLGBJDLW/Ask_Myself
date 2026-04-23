import { useMemo } from 'react';
import ReactMarkdown from 'react-markdown';
import remarkGfm from 'remark-gfm';
import { markdownComponents, rehypePlugins } from '../chat/markdownComponents';
import { useTranslation } from '../../i18n';

/**
 * Parsed YAML frontmatter fields surfaced at the top of the preview.
 * Parsing is intentionally loose: we extract a handful of known keys with
 * regexes and never fail — skills with no frontmatter still render as plain
 * markdown.
 */
interface SkillFrontmatter {
  name?: string;
  description?: string;
  triggers?: string[];
  license?: string;
  allowedTools?: string[];
  /** Any extra `key: value` pairs so we can surface them if desired. */
  extras: Array<[string, string]>;
}

interface ParsedSkillDoc {
  frontmatter: SkillFrontmatter | null;
  body: string;
}

const REMARK_PLUGINS = [remarkGfm];

/**
 * Split a full SKILL.md-style string into frontmatter and body. If the input
 * does not start with `---`, the frontmatter is `null` and the whole text is
 * treated as the body (useful when SkillEditor stores body-only content and
 * synthesises name/description separately).
 */
export function splitFrontmatter(input: string): ParsedSkillDoc {
  const text = input.replace(/^\uFEFF/, '');
  const match = text.match(/^---\r?\n([\s\S]*?)\r?\n---\r?\n?([\s\S]*)$/);
  if (!match) return { frontmatter: null, body: text };

  const [, raw, body] = match;
  const fm: SkillFrontmatter = { extras: [] };

  const lines = raw.split(/\r?\n/);
  let currentList: string[] | null = null;
  let currentListKey: 'triggers' | 'allowedTools' | null = null;

  for (const rawLine of lines) {
    const line = rawLine.replace(/\s+$/, '');
    if (!line.trim()) {
      currentList = null;
      currentListKey = null;
      continue;
    }

    // Continuation of a YAML list (`  - item`).
    const listItem = line.match(/^\s*-\s+(.*)$/);
    if (listItem && currentList) {
      currentList.push(unquote(listItem[1]));
      continue;
    }

    const kv = line.match(/^([A-Za-z0-9_-]+)\s*:\s*(.*)$/);
    if (!kv) {
      currentList = null;
      currentListKey = null;
      continue;
    }
    const key = kv[1];
    const value = kv[2].trim();

    if (key === 'name') fm.name = unquote(value);
    else if (key === 'description') fm.description = unquote(value);
    else if (key === 'license') fm.license = unquote(value);
    else if (key === 'triggers' || key === 'allowed-tools') {
      const outKey = key === 'triggers' ? 'triggers' : 'allowedTools';
      if (value.startsWith('[') && value.endsWith(']')) {
        const items = value
          .slice(1, -1)
          .split(',')
          .map((s) => unquote(s.trim()))
          .filter(Boolean);
        if (outKey === 'triggers') fm.triggers = items;
        else fm.allowedTools = items;
        currentList = null;
        currentListKey = null;
      } else {
        const arr: string[] = [];
        if (outKey === 'triggers') fm.triggers = arr;
        else fm.allowedTools = arr;
        currentList = arr;
        currentListKey = outKey;
      }
    } else if (value) {
      fm.extras.push([key, unquote(value)]);
      currentList = null;
      currentListKey = null;
    }
    void currentListKey;
  }

  return { frontmatter: fm, body: body.trim() };
}

function unquote(s: string): string {
  const trimmed = s.trim();
  if (trimmed.length >= 2) {
    const first = trimmed[0];
    const last = trimmed[trimmed.length - 1];
    if ((first === '"' && last === '"') || (first === "'" && last === "'")) {
      return trimmed.slice(1, -1);
    }
  }
  return trimmed;
}

interface SkillMarkdownPreviewProps {
  content: string;
  /** Optional fallback name/description when `content` has no frontmatter. */
  fallbackName?: string;
  fallbackDescription?: string;
  className?: string;
}

/**
 * Renders a SKILL.md (or body-only) string as a prose preview: frontmatter
 * fields go in a dedicated header strip, the body is rendered by the shared
 * chat markdown pipeline (which includes rehype-sanitize — raw HTML is
 * stripped, preventing XSS from untrusted imports).
 */
export function SkillMarkdownPreview({
  content,
  fallbackName,
  fallbackDescription,
  className,
}: SkillMarkdownPreviewProps) {
  const { t } = useTranslation();
  const parsed = useMemo(() => splitFrontmatter(content ?? ''), [content]);

  const name = parsed.frontmatter?.name ?? fallbackName ?? '';
  const description = parsed.frontmatter?.description ?? fallbackDescription ?? '';
  const triggers = parsed.frontmatter?.triggers ?? [];
  const license = parsed.frontmatter?.license;
  const allowedTools = parsed.frontmatter?.allowedTools ?? [];
  const body = parsed.body;

  const hasHeader = Boolean(
    name || description || triggers.length || license || allowedTools.length,
  );

  return (
    <div
      className={
        'rounded-md border border-border bg-surface-1 p-4 text-sm text-text-primary ' +
        (className ?? '')
      }
    >
      {hasHeader && (
        <header className="mb-4 space-y-2 border-b border-border pb-3">
          {name && (
            <h3 className="text-base font-semibold text-text-primary">{name}</h3>
          )}
          {description && (
            <p className="text-xs text-text-secondary">{description}</p>
          )}
          <dl className="flex flex-wrap gap-x-4 gap-y-1 text-[11px] text-text-tertiary">
            {triggers.length > 0 && (
              <div className="flex flex-wrap items-center gap-1">
                <dt className="font-medium text-text-secondary">
                  {t('settings.skillPreviewTriggers')}:
                </dt>
                {triggers.map((tr) => (
                  <span
                    key={tr}
                    className="inline-flex items-center rounded-full border border-border bg-surface-2 px-2 py-0.5"
                  >
                    {tr}
                  </span>
                ))}
              </div>
            )}
            {allowedTools.length > 0 && (
              <div className="flex flex-wrap items-center gap-1">
                <dt className="font-medium text-text-secondary">
                  {t('settings.skillPreviewAllowedTools')}:
                </dt>
                {allowedTools.map((tool) => (
                  <span
                    key={tool}
                    className="inline-flex items-center rounded-full border border-accent/30 bg-accent/5 px-2 py-0.5 text-accent"
                  >
                    {tool}
                  </span>
                ))}
              </div>
            )}
            {license && (
              <div>
                <dt className="inline font-medium text-text-secondary">
                  {t('settings.skillPreviewLicense')}:
                </dt>{' '}
                <dd className="inline">{license}</dd>
              </div>
            )}
          </dl>
        </header>
      )}
      {body.trim() ? (
        <div className="prose prose-sm prose-invert max-w-none text-text-primary">
          <ReactMarkdown
            remarkPlugins={REMARK_PLUGINS}
            rehypePlugins={rehypePlugins}
            components={markdownComponents}
          >
            {body}
          </ReactMarkdown>
        </div>
      ) : (
        <p className="text-xs italic text-text-tertiary">
          {t('settings.skillPreviewEmpty')}
        </p>
      )}
    </div>
  );
}
