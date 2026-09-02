import { createContext, useCallback, useContext, useEffect, useId, useRef, useState, type ComponentPropsWithoutRef, type ReactNode } from 'react';
import { Highlight, themes } from 'prism-react-renderer';
import { Copy, Check, FileText, Paperclip, ExternalLink } from 'lucide-react';
import { open } from '@tauri-apps/plugin-shell';
import DOMPurify from 'dompurify';
import remarkGfm from 'remark-gfm';
import remarkMath from 'remark-math';
import rehypeRaw from 'rehype-raw';
import rehypeKatex from 'rehype-katex';
import rehypeSanitize, { defaultSchema } from 'rehype-sanitize';
import { useTranslation } from '../../i18n';
import { openFileInDefaultApp } from '../../lib/api';
import { canPreviewInApp, useFilePreview } from '../../features/preview';
import { sourceHost } from '../../lib/sourceDisplay';
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

export interface MarkdownRenderState {
  isStreaming: boolean;
}

const MarkdownRenderStateContext = createContext<MarkdownRenderState>({ isStreaming: false });

export function MarkdownRenderStateProvider({
  isStreaming,
  children,
}: {
  isStreaming: boolean;
  children: ReactNode;
}) {
  return (
    <MarkdownRenderStateContext.Provider value={{ isStreaming }}>
      {children}
    </MarkdownRenderStateContext.Provider>
  );
}

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
  return content.replace(/\[source:\s*([^\]]+)\]/gi, (_match, path: string) => {
    const target = path.trim();
    if (/^https?:\/\//i.test(target)) {
      return `[${sourceHost(target) || target}](url:${target})`;
    }
    return `\`${target}\``;
  });
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

/** Route web links through Nexa Browser, or render local/citation references. */
function MarkdownLink({ href, children, ...rest }: ComponentPropsWithoutRef<'a'>) {
  const citationCtx = useContext(CitationContext);
  const { openFilePreview, openWebLink } = useFilePreview();

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
        onClick={() => {
          if (canPreviewInApp(filePath)) {
            openFilePreview(filePath);
          } else {
            openFileInDefaultApp(filePath);
          }
        }}
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

  // URL reference: open in the shared Nexa Browser Workspace.
  if (href && href.startsWith('url:')) {
    const rawUrl = href.slice(4);
    const host = sourceHost(rawUrl);
    const label = Array.isArray(children)
      ? children.map(String).join('')
      : String(children ?? '');
    const displayLabel = host && label && !label.includes(host)
      ? `${label} · ${host}`
      : (label || host || rawUrl);
    return (
      <button
        type="button"
        onClick={() => {
          if (/^https?:\/\//i.test(rawUrl)) {
            openWebLink(rawUrl, displayLabel);
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
        <span className="truncate max-w-[180px]">{displayLabel}</span>
      </button>
    );
  }

  const handleClick = (e: React.MouseEvent<HTMLAnchorElement>) => {
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
      if (/^https?:\/\//i.test(href)) {
        const label = Array.isArray(children)
          ? children.map(String).join('')
          : String(children ?? '');
        const displayLabel = label || sourceHost(href);
        openWebLink(href, displayLabel);
        return;
      }
      if (/^mailto:/i.test(href)) {
        open(href);
      }
  };
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
let mermaidRenderQueue: Promise<void> = Promise.resolve();
let mermaidRenderSequence = 0;

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
    secure: [
      'secure',
      'securityLevel',
      'startOnLoad',
      'maxTextSize',
      'suppressErrorRendering',
      'maxEdges',
      'htmlLabels',
      'theme',
      'themeVariables',
    ],
    suppressErrorRendering: true,
    htmlLabels: false,
    theme: 'base',
    fontFamily: 'Inter, ui-sans-serif, system-ui, sans-serif',
    themeVariables: {
      // Mermaid's base theme darkens every categorical color by 25%. Starting
      // from a dark brand color makes timeline and mind-map sections nearly
      // black while their labels remain dark. Keep the source palette light so
      // the generated SVG retains readable contrast after that transformation.
      primaryColor: '#dbeafe',
      primaryTextColor: '#0f172a',
      primaryBorderColor: '#2e75b6',
      lineColor: '#64748b',
      secondaryColor: '#e0f2fe',
      tertiaryColor: '#f8fafc',
      mainBkg: '#ffffff',
      nodeBorder: '#2e75b6',
      clusterBkg: '#f8fafc',
      clusterBorder: '#cbd5e1',
      edgeLabelBackground: '#ffffff',
      cScale0: '#dbeafe',
      cScale1: '#dcfce7',
      cScale2: '#fef3c7',
      cScale3: '#fae8ff',
      cScale4: '#ffe4e6',
      cScale5: '#e0f2fe',
      cScale6: '#ede9fe',
      cScale7: '#fce7f3',
      cScale8: '#ccfbf1',
      cScale9: '#fef9c3',
      cScale10: '#ffedd5',
      cScale11: '#e2e8f0',
      cScaleLabel0: '#0f172a',
      cScaleLabel1: '#0f172a',
      cScaleLabel2: '#0f172a',
      cScaleLabel3: '#0f172a',
      cScaleLabel4: '#0f172a',
      cScaleLabel5: '#0f172a',
      cScaleLabel6: '#0f172a',
      cScaleLabel7: '#0f172a',
      cScaleLabel8: '#0f172a',
      cScaleLabel9: '#0f172a',
      cScaleLabel10: '#0f172a',
      cScaleLabel11: '#0f172a',
    },
  });
  mermaidInitialized = true;
  return mermaid;
}


function enqueueMermaidRender<T>(task: () => Promise<T>): Promise<T> {
  const result = mermaidRenderQueue.then(task, task);
  mermaidRenderQueue = result.then(
    () => undefined,
    () => undefined,
  );
  return result;
}

type MermaidRgb = readonly [number, number, number];

function parseMermaidComputedColor(value: string): MermaidRgb | null {
  if (!value || value === 'none' || value === 'transparent') return null;
  const channels = value.match(/[\d.]+/g)?.map(Number);
  if (!channels || channels.length < 3) return null;
  if (channels.length >= 4 && channels[3] === 0) return null;
  return [channels[0], channels[1], channels[2]];
}

function mermaidRelativeLuminance(color: MermaidRgb): number {
  const channels = color.map((channel) => {
    const normalized = channel / 255;
    return normalized <= 0.04045
      ? normalized / 12.92
      : ((normalized + 0.055) / 1.055) ** 2.4;
  });
  return channels[0] * 0.2126 + channels[1] * 0.7152 + channels[2] * 0.0722;
}

function mermaidContrastRatio(background: MermaidRgb, foreground: MermaidRgb): number {
  const backgroundLuminance = mermaidRelativeLuminance(background);
  const foregroundLuminance = mermaidRelativeLuminance(foreground);
  const lighter = Math.max(backgroundLuminance, foregroundLuminance);
  const darker = Math.min(backgroundLuminance, foregroundLuminance);
  return (lighter + 0.05) / (darker + 0.05);
}

function enforceReadableMermaidNodePalette(root: Element): void {
  const probeHost = document.createElement('div');
  probeHost.style.cssText = [
    'all: initial',
    'position: fixed',
    'left: -100000px',
    'top: 0',
    'width: 1200px',
    'visibility: hidden',
    'pointer-events: none',
  ].join(';');
  document.body.appendChild(probeHost);
  probeHost.appendChild(root);

  const safeFills = [
    '#dbeafe',
    '#dcfce7',
    '#fef3c7',
    '#fae8ff',
    '#ffe4e6',
    '#e0f2fe',
  ];
  try {
    root.querySelectorAll<SVGGElement>('g.node').forEach((node, index) => {
      const shape = node.querySelector<SVGElement>('rect, polygon, circle, ellipse, path');
      const labelProbe = node.querySelector<SVGElement>('text, tspan, .nodeLabel, .label');
      if (!shape || !labelProbe) return;

      const background = parseMermaidComputedColor(getComputedStyle(shape).fill);
      const labelStyle = getComputedStyle(labelProbe);
      const foreground = parseMermaidComputedColor(labelStyle.fill)
        ?? parseMermaidComputedColor(labelStyle.color);
      if (!background || !foreground || mermaidContrastRatio(background, foreground) >= 4.5) {
        return;
      }

      shape.style.setProperty('fill', safeFills[index % safeFills.length], 'important');
      shape.style.setProperty('stroke', '#2e75b6', 'important');
      node.style.setProperty('color', '#0f172a', 'important');
      node.querySelectorAll<SVGElement>('text, tspan, .nodeLabel, .label').forEach((label) => {
        label.style.setProperty('fill', '#0f172a', 'important');
        label.style.setProperty('color', '#0f172a', 'important');
      });
    });
    materializeMermaidPresentationAttributes(root);
  } finally {
    probeHost.remove();
  }
}

const MERMAID_PRESENTATION_PROPERTIES = [
  'color',
  'fill',
  'fill-opacity',
  'fill-rule',
  'opacity',
  'paint-order',
  'stroke',
  'stroke-dasharray',
  'stroke-dashoffset',
  'stroke-linecap',
  'stroke-linejoin',
  'stroke-miterlimit',
  'stroke-opacity',
  'stroke-width',
] as const;

const MERMAID_TEXT_PRESENTATION_PROPERTIES = [
  'alignment-baseline',
  'baseline-shift',
  'dominant-baseline',
  'font-family',
  'font-size',
  'font-style',
  'font-weight',
  'letter-spacing',
  'text-anchor',
  'text-decoration',
  'text-rendering',
  'word-spacing',
] as const;

/**
 * Tauri production builds hash static inline styles for CSP. Mermaid SVGs are
 * rendered at runtime and then inserted through innerHTML, so their generated
 * <style> element and style attributes are intentionally not part of that
 * static allow-list. WebView keeps the attributes in outerHTML but declines to
 * apply them, leaving SVG's black fill and start-aligned text defaults.
 *
 * SVG presentation attributes are data, not executable CSS. Materialize the
 * already-sanitized computed palette and typography while the SVG is mounted
 * in the isolated probe host so the same geometry survives strict production
 * CSP without weakening the application policy.
 */
function materializeMermaidPresentationAttributes(root: Element): void {
  const svgNamespace = 'http://www.w3.org/2000/svg';
  const elements = [root, ...root.querySelectorAll('*')]
    .filter((element): element is SVGElement => element.namespaceURI === svgNamespace);

  elements.forEach((element) => {
    if (element.localName.toLowerCase() === 'style') return;
    const computed = getComputedStyle(element);
    MERMAID_PRESENTATION_PROPERTIES.forEach((property) => {
      const value = computed.getPropertyValue(property).trim();
      if (value) element.setAttribute(property, value);
    });

    if (['text', 'tspan', 'textpath'].includes(element.localName.toLowerCase())) {
      MERMAID_TEXT_PRESENTATION_PROPERTIES.forEach((property) => {
        const value = computed.getPropertyValue(property).trim();
        if (value) element.setAttribute(property, value);
      });
    }

    if (element.localName.toLowerCase() === 'stop') {
      for (const property of ['stop-color', 'stop-opacity'] as const) {
        const value = computed.getPropertyValue(property).trim();
        if (value) element.setAttribute(property, value);
      }
    }
  });
}

export function sanitizeMermaidSvg(svg: string): string {
  // Mermaid needs its generated <style> element for its palette. The SVG-only
  // DOMPurify profile removes it and leaves black-on-black browser defaults.
  // Labels are generated as pure SVG text (htmlLabels: false), while this mixed
  // profile preserves styles and still blocks remote CSS, images, and links.
  const localOnlySvg = svg
    .replace(/@import\s+[^;]+;/gi, '')
    .replace(/url\(\s*(?!['"]?#)[^)]+\)/gi, 'none');

  const sanitized = String(DOMPurify.sanitize(localOnlySvg, {
    USE_PROFILES: { html: true, svg: true, svgFilters: true },
  }));

  // Mermaid SVGs may contain HTML labels inside foreignObject. Parsing that
  // browser-valid mixed markup as XML rejects diagrams containing elements
  // such as <br>; use the browser's HTML parser, matching how React inserts it.
  const template = document.createElement('template');
  template.innerHTML = sanitized;
  const root = template.content.firstElementChild;
  if (!root || root.tagName.toLowerCase() !== 'svg') return '';
  const existingStyle = root.getAttribute('style')?.trim();
  root.setAttribute(
    'style',
    [
      existingStyle,
      'font-family: Inter, ui-sans-serif, system-ui, sans-serif',
      'font-size: 16px',
      'line-height: normal',
      'letter-spacing: normal',
      'color: #0f172a',
      'color-scheme: light',
    ].filter(Boolean).join('; '),
  );

  root.querySelectorAll('*').forEach((element) => {
    for (const name of ['href', 'xlink:href']) {
      const value = element.getAttribute(name);
      if (value && !value.trim().startsWith('#')) {
        element.removeAttribute(name);
      }
    }
  });

  root.querySelectorAll('style').forEach((style) => {
    style.textContent = (style.textContent ?? '')
      .replace(/@import\s+[^;]+;/gi, '')
      .replace(/url\(\s*(?!['"]?#)[^)]+\)/gi, 'none');
  });

  // Mermaid directives and classDef rules are model-authored content. Keep
  // valid custom palettes, but repair the exact unreadable case where a node
  // fill and its label resolve below WCAG AA contrast (including WebView style
  // loss, whose SVG defaults are black fill plus black text).
  enforceReadableMermaidNodePalette(root);

  return root.outerHTML;
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

export function repairMermaidFlowchartLabels(chart: string): string {
  if (!/(?:^|\n)\s*(?:flowchart|graph)\s+(?:TB|TD|BT|RL|LR)\b/i.test(chart)) {
    return chart;
  }

  // Mermaid's flowchart grammar treats parentheses in bare square-bracket
  // labels as shape syntax. Formula-heavy labels such as
  // `C[25×(SV/ST+RSV/RST)/2]` therefore fail even though the intent is plain
  // text. Quoted labels are the grammar-supported equivalent and also retain
  // line breaks such as <br/> when Mermaid emits pure SVG labels.
  return chart.replace(
    /(\b[A-Za-z_][\w-]*)\[([^\]\r\n]*)\]/g,
    (match, nodeId: string, rawLabel: string) => {
      const label = rawLabel.trim();
      if (!label || (label.startsWith('"') && label.endsWith('"'))) return match;
      return `${nodeId}["${label.replace(/"/g, '&quot;')}"]`;
    },
  );
}

export function MermaidBlock({ chart }: { chart: string }) {
  const { t } = useTranslation();
  const { isStreaming } = useContext(MarkdownRenderStateContext);
  const [copied, setCopied] = useState(false);
  const [svg, setSvg] = useState('');
  const [renderState, setRenderState] = useState<'rendering' | 'ready' | 'deferred' | 'invalid'>(
    'rendering',
  );
  const diagramId = useId().replace(/[:]/g, '-');
  const renderGeneration = useRef(0);
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
    const generation = ++renderGeneration.current;
    let cancelled = false;
    const isCurrent = () => !cancelled && renderGeneration.current === generation;

    // Parsing and laying out an incomplete Mermaid program can monopolize the
    // browser main thread and enqueue stale work on every text delta. Keep the
    // source available, but render the diagram exactly once after streaming.
    if (isStreaming) {
      setSvg('');
      setRenderState('deferred');
      return () => {
        cancelled = true;
      };
    }

    const render = async () => {
      if (!normalizedChart) {
        if (isCurrent()) {
          setSvg('');
          setRenderState('invalid');
        }
        return;
      }

      try {
        const mermaid = await loadMermaid();
        if (!isCurrent()) return;
        setSvg('');
        setRenderState('rendering');

        const renderId = `mermaid-${diagramId}-${++mermaidRenderSequence}`;
        const rendered = await enqueueMermaidRender(async () => {
          if (!isCurrent()) return null;
          const renderHost = document.createElement('div');
          renderHost.style.cssText = [
            'all: initial',
            'display: block',
            'position: fixed',
            'left: -100000px',
            'top: 0',
            'width: 1200px',
            'font-family: Inter, ui-sans-serif, system-ui, sans-serif',
            'font-size: 16px',
            'line-height: normal',
            'letter-spacing: normal',
            'color: #0f172a',
            'color-scheme: light',
          ].join(';');
          document.body.appendChild(renderHost);
          try {
            const repairedChart = repairMermaidFlowchartLabels(normalizedChart);
            const candidates = repairedChart === normalizedChart
              ? [normalizedChart]
              : [normalizedChart, repairedChart];

            for (const candidate of candidates) {
              try {
                const parsed = await mermaid.parse(candidate, { suppressErrors: true });
                if (!parsed || !isCurrent()) continue;
                return await mermaid.render(renderId, candidate, renderHost);
              } catch {
                document.getElementById(renderId)?.remove();
              }
            }
            return null;
          } finally {
            document.getElementById(renderId)?.remove();
            renderHost.remove();
          }
        });
        if (!rendered) {
          if (isCurrent()) {
            setRenderState('invalid');
          }
          return;
        }

        if (isCurrent()) {
          const sanitizedSvg = sanitizeMermaidSvg(rendered.svg);
          setSvg(sanitizedSvg);
          setRenderState(sanitizedSvg ? 'ready' : 'invalid');
        }
      } catch {
        if (isCurrent()) {
          setSvg('');
          setRenderState('invalid');
        }
      }
    };

    void render();
    return () => {
      cancelled = true;
    };
  }, [normalizedChart, diagramId, isStreaming]);

  return (
    <div className="group/code relative my-2 overflow-hidden rounded-lg border border-border bg-surface-1/70">
      <div className="flex items-center justify-between border-b border-border/60 bg-surface-2/80 px-3 py-2">
        <span className="text-[11px] font-medium uppercase tracking-[0.12em] text-text-tertiary">
          {t('chat.mermaidDiagram')}
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

      <div
        className="mermaid-surface overflow-x-auto bg-white px-3 py-3 text-slate-900"
        data-testid="mermaid-surface"
        style={{
          colorScheme: 'light',
          fontFamily: 'Inter, ui-sans-serif, system-ui, sans-serif',
          fontSize: '16px',
          lineHeight: 'normal',
          letterSpacing: 'normal',
        }}
      >
        {svg && renderState === 'ready' ? (
          <div
            className="[&_svg]:mx-auto [&_svg]:h-auto [&_svg]:max-w-full"
            dangerouslySetInnerHTML={{ __html: svg }}
          />
        ) : renderState === 'rendering' ? (
          <div className="py-6 text-center text-xs text-slate-500">{t('chat.mermaidRendering')}</div>
        ) : renderState === 'deferred' ? (
          <div className="space-y-2">
            <div className="rounded-md border border-slate-200 bg-slate-50 px-3 py-2 text-xs text-slate-600">
              {t('chat.mermaidWaiting')}
            </div>
            <pre className="max-h-64 overflow-x-auto rounded-md border border-slate-200 bg-slate-50 px-3 py-2 text-xs text-slate-700">
              <code>{normalizedChart || chart}</code>
            </pre>
          </div>
        ) : (
          <div className="space-y-2">
            <div className="rounded-md border border-amber-200 bg-amber-50 px-3 py-2 text-xs text-amber-800">
              {t('chat.mermaidFailed')}
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
  attributes: {
    ...defaultSchema.attributes,
    code: [
      ...(defaultSchema.attributes?.code || []),
      ['className', 'language-math', 'math-inline', 'math-display'],
    ],
  },
  protocols: {
    ...defaultSchema.protocols,
    href: [...(defaultSchema.protocols?.href || []), 'cite', 'doc', 'file', 'url'],
  },
  clobber: [],
};

/** Pre-built remark plugin list for ReactMarkdown */
// eslint-disable-next-line @typescript-eslint/no-explicit-any
export const markdownRemarkPlugins: any[] = [remarkGfm, remarkMath];

/** Pre-built rehype plugin list for ReactMarkdown */
// eslint-disable-next-line @typescript-eslint/no-explicit-any
export const rehypePlugins: any[] = [
  rehypeRaw,
  [rehypeSanitize, sanitizeSchema],
  [rehypeKatex, { throwOnError: false, strict: 'warn', trust: false, output: 'html' }],
];

/** Shared markdown component map for ReactMarkdown */
export const markdownComponents: Record<string, React.ComponentType<ComponentPropsWithoutRef<any>>> = {
  a: MarkdownLink,
  pre({ children, ...rest }: ComponentPropsWithoutRef<'pre'>) {
    // Let CodeBlock handle its own <pre>; avoid double-wrapping
    const child = children as React.ReactElement<{ className?: string }> | undefined;
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
