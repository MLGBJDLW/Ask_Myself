import { createContext, useCallback, useContext, useEffect, useId, useState, type ComponentPropsWithoutRef } from 'react';
import { Highlight, themes } from 'prism-react-renderer';
import { Copy, Check, FileText, Paperclip, ExternalLink } from 'lucide-react';
import { open } from '@tauri-apps/plugin-shell';
import rehypeRaw from 'rehype-raw';
import rehypeSanitize, { defaultSchema } from 'rehype-sanitize';
import { useTranslation } from '../../i18n';
import { openFileInDefaultApp } from '../../lib/api';
import { FileBadge } from '../ui/FileBadge';
import { CitationChip } from './EvidenceCard';
import type { CitationCardData } from '../../lib/citationParser';

type MermaidModule = typeof import('mermaid');

/* ------------------------------------------------------------------ */
/*  Citation context — provides chunk_id → evidence data lookup        */
/* ------------------------------------------------------------------ */

export interface CitationLookup {
  getCard(chunkId: string): CitationCardData | undefined;
}

const defaultLookup: CitationLookup = { getCard: () => undefined };

export const CitationContext = createContext<CitationLookup>(defaultLookup);

/* ------------------------------------------------------------------ */
/*  File-path detection constants                                      */
/* ------------------------------------------------------------------ */

const FILE_EXT =
  'md|markdown|txt|log|pdf|docx|xlsx|xls|pptx|ts|tsx|js|jsx|rs|' +
  'json|toml|yaml|yml|css|scss|sass|less|html|py|go|java|c|cpp|' +
  'h|hpp|sh|bat|sql|xml|csv';

const FILE_PATH_REGEX = new RegExp(
  `^(?:[A-Za-z]:[\\\\/]|\\.{1,2}[\\\\/]|\\/|[\\w.-]+[\\\\/])?[\\w .,()\\\\/~\\-\\u4e00-\\u9fff]*\\.(?:${FILE_EXT})$`,
  'i',
);

/* ------------------------------------------------------------------ */
/*  Markdown preprocessing                                             */
/* ------------------------------------------------------------------ */

/**
 * Pre-process AI citations like [source: D:\path\to\file.docx]
 * into backtick-wrapped paths so the `code` component renders them as FileBadge.
 */
export function preprocessCitations(content: string): string {
  return content.replace(/\[source:\s*([^\]]+)\]/gi, (_match, path: string) => `\`${path.trim()}\``);
}

/**
 * Detects bare file paths in markdown prose and wraps them in backticks
 * so they get rendered as FileBadge components by the code component.
 * Uses a 3-phase protect→match→restore approach to avoid breaking
 * existing markdown constructs.
 */
export function preprocessFilePaths(content: string): string {
  // Phase 1: Protect constructs that must not be modified
  const saved: string[] = [];
  const protect = (m: string) => {
    saved.push(m);
    return `\x00${saved.length - 1}\x00`;
  };

  let s = content
    .replace(/```[\s\S]*?```/g, protect)                // fenced code blocks
    .replace(/`[^`\n]+`/g, protect)                      // inline code (already wrapped)
    .replace(/!\[[^\]]*\]\([^)]*\)/g, protect)           // image links
    .replace(/\[[^\]]*\]\([^)]*\)/g, protect)            // markdown links
    .replace(/\[[^\]]*\]\[[^\]]*\]/g, protect)           // reference links
    .replace(/(?:https?|ftp):\/\/[^\s)>\]]+/gi, protect); // URLs

  // Phase 2: Wrap bare file paths in backticks
  const withSep =
    `(?:[A-Za-z]:[/\\\\]|\\.{1,2}[/\\\\]|[\\w\\-][\\w.\\-]*[/\\\\])` +
    `(?:[\\w .,()/\\\\~\\-\\u4e00-\\u9fff])*` +
    `\\.(?:${FILE_EXT})`;

  const bare = `[\\w][\\w.\\-]*\\.(?:${FILE_EXT})`;

  const filePathRx = new RegExp(
    `(?<![\\w\`/\\\\])(?:${withSep}|${bare})(?![\\w/\\\\]|\\.\\w)`,
    'gi',
  );

  s = s.replace(filePathRx, '`$&`');

  // Phase 3: Restore protected constructs
  return s.replace(/\x00(\d+)\x00/g, (_, i) => saved[+i]);
}

function scrollAnchorIntoChatContainer(target: HTMLElement): boolean {
  const scrollRoot = target.closest('[data-chat-scroll-root="true"]');
  if (!(scrollRoot instanceof HTMLElement)) {
    return false;
  }

  const rootRect = scrollRoot.getBoundingClientRect();
  const targetRect = target.getBoundingClientRect();
  const targetTop = scrollRoot.scrollTop + (targetRect.top - rootRect.top);
  const nextTop = Math.max(
    0,
    Math.min(targetTop - 24, scrollRoot.scrollHeight - scrollRoot.clientHeight),
  );

  scrollRoot.scrollTo({ top: nextTop, behavior: 'smooth' });
  return true;
}

/* ------------------------------------------------------------------ */
/*  Markdown component overrides                                       */
/* ------------------------------------------------------------------ */

/** Open links in the system browser via Tauri shell, or render citation chips */
function MarkdownLink({ href, children, ...rest }: ComponentPropsWithoutRef<'a'>) {
  const citationCtx = useContext(CitationContext);

  // Detect citation links: href="cite:CHUNK_ID"
  if (href && href.startsWith('cite:')) {
    const chunkId = href.slice(5); // strip "cite:"
    const displayText = typeof children === 'string'
      ? children
      : Array.isArray(children)
        ? children.map(String).join('')
        : String(children ?? '');
    const card = citationCtx.getCard(chunkId);
    return <CitationChip chunkId={chunkId} displayText={displayText} card={card} />;
  }

  // Document reference badge
  if (href && href.startsWith('doc:')) {
    const docId = href.slice(4);
    return (
      <span
        className="inline-flex items-center gap-0.5 px-1.5 py-0 text-[11px] font-medium
          rounded-full border cursor-default transition-all duration-150
          bg-blue-500/10 text-blue-600 dark:text-blue-400 border-blue-500/20
          align-baseline leading-[1.4] mx-0.5"
        title={docId}
      >
        <FileText className="h-2.5 w-2.5 shrink-0" />
        <span className="truncate max-w-[150px]">{children}</span>
      </span>
    );
  }

  // File reference: open in default app
  if (href && href.startsWith('file:')) {
    const filePath = href.slice(5);
    return (
      <button
        type="button"
        onClick={() => openFileInDefaultApp(filePath)}
        className="inline-flex items-center gap-0.5 px-1.5 py-0 text-[11px] font-medium
          rounded-full border cursor-pointer transition-all duration-150
          bg-emerald-500/10 text-emerald-600 dark:text-emerald-400 border-emerald-500/20
          hover:bg-emerald-500/20 hover:border-emerald-500/30
          active:scale-95 align-baseline leading-[1.4] mx-0.5"
        title={filePath}
      >
        <Paperclip className="h-2.5 w-2.5 shrink-0" />
        <span className="truncate max-w-[150px]">{children}</span>
      </button>
    );
  }

  // URL reference: open in system browser
  if (href && href.startsWith('url:')) {
    const rawUrl = href.slice(4);
    return (
      <button
        type="button"
        onClick={() => {
          if (/^https?:\/\//i.test(rawUrl)) {
            open(rawUrl);
          }
        }}
        className="inline-flex items-center gap-0.5 px-1.5 py-0 text-[11px] font-medium
          rounded-full border cursor-pointer transition-all duration-150
          bg-orange-500/10 text-orange-600 dark:text-orange-400 border-orange-500/20
          hover:bg-orange-500/20 hover:border-orange-500/30
          active:scale-95 align-baseline leading-[1.4] mx-0.5"
        title={rawUrl}
      >
        <ExternalLink className="h-2.5 w-2.5 shrink-0" />
        <span className="truncate max-w-[150px]">{children}</span>
      </button>
    );
  }

  const handleClick = useCallback(
    (e: React.MouseEvent<HTMLAnchorElement>) => {
      e.preventDefault();
      if (!href) return;

      // Keep in-page anchors (e.g. GFM footnotes) navigable.
      if (href.startsWith('#')) {
        e.preventDefault();
        const rawId = decodeURIComponent(href.slice(1));
        if (!rawId) return;
        const candidateIds = [rawId, `user-content-${rawId.replace(/^user-content-/, '')}`];
        for (const id of candidateIds) {
          const target = document.getElementById(id);
          if (target) {
            if (!scrollAnchorIntoChatContainer(target)) {
              target.scrollIntoView({ behavior: 'smooth', block: 'start' });
            }
            return;
          }
        }
        return;
      }

      e.preventDefault();
      open(href);
    },
    [href],
  );
  return (
    <a
      {...rest}
      href={href}
      onClick={handleClick}
      className="text-accent hover:text-accent-hover underline underline-offset-2"
    >
      {children}
    </a>
  );
}

/** Fenced code block with syntax highlighting and copy button */
function CodeBlock({ code, language }: { code: string; language: string }) {
  const { t } = useTranslation();
  const [copied, setCopied] = useState(false);

  const handleCopy = useCallback(async () => {
    try {
      await navigator.clipboard.writeText(code);
      setCopied(true);
      setTimeout(() => setCopied(false), 2000);
    } catch {
      // Silently fail if clipboard access is denied
    }
  }, [code]);

  return (
    <div className="group/code relative my-2">
      <button
        type="button"
        onClick={handleCopy}
        title={copied ? t('chat.copied') : t('chat.copyCode')}
        className="absolute top-2 right-2 z-10 flex items-center gap-1 px-1.5 py-0.5 rounded text-[11px]
          bg-surface-0/60 border border-border/40 text-text-tertiary
          opacity-0 group-hover/code:opacity-100
          hover:bg-surface-0 hover:text-text-primary hover:border-border
          transition-all duration-150 cursor-pointer select-none"
      >
        {copied ? (
          <>
            <Check className="h-3 w-3 text-green-500" />
            <span className="text-green-500">{t('chat.copied')}</span>
          </>
        ) : (
          <>
            <Copy className="h-3 w-3" />
            <span>{t('chat.copyCode')}</span>
          </>
        )}
      </button>
      <Highlight theme={themes.oneDark} code={code} language={language}>
        {({ tokens, getLineProps, getTokenProps }) => (
          <pre className="bg-surface-0 border border-border rounded-md px-3 py-2 text-xs overflow-x-auto">
            <code>
              {tokens.map((line, i) => (
                <div key={i} {...getLineProps({ line })}>
                  {line.map((token, key) => (
                    <span key={key} {...getTokenProps({ token })} />
                  ))}
                </div>
              ))}
            </code>
          </pre>
        )}
      </Highlight>
    </div>
  );
}

let mermaidInitialized = false;
let mermaidModulePromise: Promise<MermaidModule> | null = null;

async function loadMermaid() {
  if (!mermaidModulePromise) {
    mermaidModulePromise = import('mermaid');
  }
  const module = await mermaidModulePromise;
  const mermaid = module.default;
  if (mermaidInitialized) return mermaid;

  mermaid.initialize({
    startOnLoad: false,
    securityLevel: 'strict',
    theme: 'base',
    fontFamily: 'Inter, ui-sans-serif, system-ui, sans-serif',
    themeVariables: {
      primaryColor: '#1f4e79',
      primaryTextColor: '#0f172a',
      primaryBorderColor: '#2e75b6',
      lineColor: '#64748b',
      secondaryColor: '#eaf3fb',
      tertiaryColor: '#f8fafc',
      mainBkg: '#ffffff',
      nodeBorder: '#2e75b6',
      clusterBkg: '#f8fafc',
      clusterBorder: '#cbd5e1',
      edgeLabelBackground: '#ffffff',
    },
  });
  mermaidInitialized = true;
  return mermaid;
}

export function normalizeMermaidChart(chart: string): string {
  let normalized = chart
    .replace(/^\uFEFF/, '')
    .replace(/\r\n?/g, '\n')
    .trim();

  const fenced = normalized.match(/^```(?:\s*mermaid)?[^\n]*\n([\s\S]*?)\n```\s*$/i);
  if (fenced) {
    normalized = fenced[1].trim();
  }

  return normalized.replace(/^\s*mermaid\s*\n/i, '').trim();
}

function MermaidBlock({ chart }: { chart: string }) {
  const { t } = useTranslation();
  const [copied, setCopied] = useState(false);
  const [svg, setSvg] = useState('');
  const [rendering, setRendering] = useState(true);
  const diagramId = useId().replace(/[:]/g, '-');
  const normalizedChart = normalizeMermaidChart(chart);

  const handleCopy = useCallback(async () => {
    try {
      await navigator.clipboard.writeText(normalizedChart);
      setCopied(true);
      setTimeout(() => setCopied(false), 2000);
    } catch {
      // ignore clipboard failures
    }
  }, [normalizedChart]);

  useEffect(() => {
    let cancelled = false;

    const render = async () => {
      try {
        const mermaid = await loadMermaid();
        setRendering(true);
        const { svg: nextSvg } = await mermaid.render(
          `mermaid-${diagramId}-${Date.now()}`,
          normalizedChart,
        );
        if (!cancelled) {
          setSvg(nextSvg);
        }
      } catch {
        if (!cancelled) {
          setSvg('');
        }
      } finally {
        if (!cancelled) {
          setRendering(false);
        }
      }
    };

    void render();
    return () => {
      cancelled = true;
    };
  }, [normalizedChart, diagramId]);

  return (
    <div className="group/code relative my-2 overflow-hidden rounded-lg border border-border bg-surface-1/70">
      <div className="flex items-center justify-between border-b border-border/60 bg-surface-2/80 px-3 py-2">
        <span className="text-[11px] font-medium uppercase tracking-[0.12em] text-text-tertiary">
          Mermaid
        </span>
        <button
          type="button"
          onClick={handleCopy}
          title={copied ? t('chat.copied') : t('chat.copyCode')}
          className="flex items-center gap-1 rounded border border-border/40 bg-surface-0/60 px-1.5 py-0.5 text-[11px] text-text-tertiary transition-all duration-150 hover:border-border hover:bg-surface-0 hover:text-text-primary cursor-pointer"
        >
          {copied ? (
            <>
              <Check className="h-3 w-3 text-green-500" />
              <span className="text-green-500">{t('chat.copied')}</span>
            </>
          ) : (
            <>
              <Copy className="h-3 w-3" />
              <span>{t('chat.copyCode')}</span>
            </>
          )}
        </button>
      </div>

      <div className="overflow-x-auto bg-white px-3 py-3">
        {svg ? (
          <div
            className="[&_svg]:mx-auto [&_svg]:h-auto [&_svg]:max-w-full"
            dangerouslySetInnerHTML={{ __html: svg }}
          />
        ) : rendering ? (
          <div className="py-6 text-center text-xs text-slate-500">Rendering diagram...</div>
        ) : (
          <div className="space-y-2">
            <div className="rounded-md border border-amber-200 bg-amber-50 px-3 py-2 text-xs text-amber-800">
              Could not render this Mermaid diagram. The source is shown below.
            </div>
            <pre className="overflow-x-auto rounded-md border border-slate-200 bg-slate-50 px-3 py-2 text-xs text-slate-700">
              <code>{normalizedChart || chart}</code>
            </pre>
          </div>
        )}
      </div>
    </div>
  );
}

/**
 * Sanitize schema for rehype-sanitize: allows common formatting HTML
 * but blocks dangerous elements (script, iframe, form, object, embed, style, link).
 */
export const sanitizeSchema = {
  ...defaultSchema,
  tagNames: [...(defaultSchema.tagNames || []), 'br', 'sub', 'sup', 'mark', 'kbd', 'abbr', 'details', 'summary'],
  protocols: {
    ...defaultSchema.protocols,
    href: [...(defaultSchema.protocols?.href || []), 'cite', 'doc', 'file', 'url'],
  },
  clobber: [],
};

/** Pre-built rehype plugin list for ReactMarkdown */
// eslint-disable-next-line @typescript-eslint/no-explicit-any
export const rehypePlugins: any[] = [rehypeRaw, [rehypeSanitize, sanitizeSchema]];

/** Shared markdown component map for ReactMarkdown */
export const markdownComponents: Record<string, React.ComponentType<ComponentPropsWithoutRef<any>>> = {
  a: MarkdownLink,
  pre({ children, ...rest }: ComponentPropsWithoutRef<'pre'>) {
    // Let CodeBlock handle its own <pre>; avoid double-wrapping
    const child = children as React.ReactElement | undefined;
    if (child?.props?.className?.startsWith('language-')) {
      return <>{children}</>;
    }
    return (
      <pre
        {...rest}
        className="bg-surface-0 border border-border rounded-md px-3 py-2 my-2 text-xs overflow-x-auto"
      >
        {children}
      </pre>
    );
  },
  code({ children, className, ...rest }: ComponentPropsWithoutRef<'code'> & { className?: string }) {
    const language = className?.replace('language-', '') ?? '';
    const isBlock = className?.startsWith('language-');

    if (isBlock) {
      // Extract raw text from children
      const raw = typeof children === 'string'
        ? children
        : Array.isArray(children)
          ? children.join('')
          : String(children ?? '');
      // Remove trailing newline that react-markdown adds
      const code = raw.replace(/\n$/, '');
      if (language.toLowerCase() === 'mermaid') {
        return <MermaidBlock chart={code} />;
      }
      return <CodeBlock code={code} language={language} />;
    }

    // Detect file paths in inline code and render as FileBadge
    const text = typeof children === 'string' ? children : Array.isArray(children) ? children.join('') : '';
    if (
      typeof text === 'string' &&
      text.length > 0 &&
      FILE_PATH_REGEX.test(text)
    ) {
      return <FileBadge path={text} />;
    }
    return (
      <code
        {...rest}
        className="bg-surface-0 border border-border rounded px-1 py-0.5 text-xs"
      >
        {children}
      </code>
    );
  },
  h1({ children, ...r }: ComponentPropsWithoutRef<'h1'>) {
    return <h1 {...r} className="text-xl font-bold mt-4 mb-2">{children}</h1>;
  },
  h2({ children, ...r }: ComponentPropsWithoutRef<'h2'>) {
    return <h2 {...r} className="text-lg font-semibold mt-3 mb-1.5">{children}</h2>;
  },
  h3({ children, ...r }: ComponentPropsWithoutRef<'h3'>) {
    return <h3 {...r} className="text-base font-semibold mt-3 mb-1">{children}</h3>;
  },
  h4({ children, ...r }: ComponentPropsWithoutRef<'h4'>) {
    return <h4 {...r} className="text-sm font-semibold mt-2 mb-1">{children}</h4>;
  },
  ul({ children, ...r }: ComponentPropsWithoutRef<'ul'>) {
    return <ul {...r} className="list-disc list-inside my-1.5 space-y-0.5">{children}</ul>;
  },
  ol({ children, ...r }: ComponentPropsWithoutRef<'ol'>) {
    return <ol {...r} className="list-decimal list-inside my-1.5 space-y-0.5">{children}</ol>;
  },
  li({ children, ...r }: ComponentPropsWithoutRef<'li'>) {
    return <li {...r} className="leading-relaxed">{children}</li>;
  },
  blockquote({ children, ...r }: ComponentPropsWithoutRef<'blockquote'>) {
    return (
      <blockquote
        {...r}
        className="border-l-2 border-accent/40 pl-3 my-2 text-text-secondary italic"
      >
        {children}
      </blockquote>
    );
  },
  table({ children, ...r }: ComponentPropsWithoutRef<'table'>) {
    return (
      <div className="overflow-x-auto my-2">
        <table {...r} className="min-w-full text-xs border border-border rounded-md">
          {children}
        </table>
      </div>
    );
  },
  thead({ children, ...r }: ComponentPropsWithoutRef<'thead'>) {
    return <thead {...r} className="bg-surface-3">{children}</thead>;
  },
  th({ children, ...r }: ComponentPropsWithoutRef<'th'>) {
    return (
      <th {...r} className="px-2 py-1 text-left font-medium border-b border-border">
        {children}
      </th>
    );
  },
  td({ children, ...r }: ComponentPropsWithoutRef<'td'>) {
    return (
      <td {...r} className="px-2 py-1 border-b border-border">
        {children}
      </td>
    );
  },
  tr({ children, ...r }: ComponentPropsWithoutRef<'tr'>) {
    return <tr {...r} className="even:bg-surface-0/30">{children}</tr>;
  },
  hr(r: ComponentPropsWithoutRef<'hr'>) {
    return <hr {...r} className="border-border my-3" />;
  },
  p({ children, ...r }: ComponentPropsWithoutRef<'p'>) {
    return <p {...r} className="my-1.5 leading-relaxed">{children}</p>;
  },
  strong({ children, ...r }: ComponentPropsWithoutRef<'strong'>) {
    return <strong {...r} className="font-semibold">{children}</strong>;
  },
};
