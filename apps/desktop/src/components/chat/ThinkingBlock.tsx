import { useState, useRef, useEffect, type ComponentPropsWithoutRef } from 'react';
import { motion, AnimatePresence } from 'framer-motion';
import { ChevronRight, Brain } from 'lucide-react';
import ReactMarkdown from 'react-markdown';
import remarkGfm from 'remark-gfm';
import { useTranslation } from '../../i18n';

/* ------------------------------------------------------------------ */
/*  Types                                                              */
/* ------------------------------------------------------------------ */

interface ThinkingBlockProps {
  content: string;
  isStreaming?: boolean;
}

/* ------------------------------------------------------------------ */
/*  Minimal markdown overrides (muted style)                           */
/* ------------------------------------------------------------------ */

const thinkingMarkdownComponents: Record<string, React.ComponentType<ComponentPropsWithoutRef<any>>> = {
  p({ children, ...r }: ComponentPropsWithoutRef<'p'>) {
    return <p {...r} className="my-1 leading-relaxed">{children}</p>;
  },
  pre({ children, ...rest }: ComponentPropsWithoutRef<'pre'>) {
    return (
      <pre
        {...rest}
        className="bg-surface-0/50 border border-border/50 rounded-md px-2.5 py-1.5 my-1.5 text-xs overflow-x-auto"
      >
        {children}
      </pre>
    );
  },
  code({ children, className, ...rest }: ComponentPropsWithoutRef<'code'> & { className?: string }) {
    const isBlock = className?.startsWith('language-');
    if (isBlock) {
      return <code {...rest} className={className}>{children}</code>;
    }
    return (
      <code {...rest} className="bg-surface-0/50 border border-border/50 rounded px-1 py-0.5 text-xs">
        {children}
      </code>
    );
  },
  ul({ children, ...r }: ComponentPropsWithoutRef<'ul'>) {
    return <ul {...r} className="list-disc list-inside my-1 space-y-0.5">{children}</ul>;
  },
  ol({ children, ...r }: ComponentPropsWithoutRef<'ol'>) {
    return <ol {...r} className="list-decimal list-inside my-1 space-y-0.5">{children}</ol>;
  },
  blockquote({ children, ...r }: ComponentPropsWithoutRef<'blockquote'>) {
    return (
      <blockquote {...r} className="border-l-2 border-text-tertiary/30 pl-2.5 my-1.5 italic opacity-80">
        {children}
      </blockquote>
    );
  },
};

/* ------------------------------------------------------------------ */
/*  Component                                                          */
/* ------------------------------------------------------------------ */

export function ThinkingBlock({ content, isStreaming = false }: ThinkingBlockProps) {
  const { t } = useTranslation();
  const [expanded, setExpanded] = useState(isStreaming);
  const startTimeRef = useRef<number>(Date.now());
  const autoOpenedRef = useRef(false);
  const [elapsed, setElapsed] = useState(0);

  // Auto-open once when live reasoning content starts arriving.
  useEffect(() => {
    const hasContent = content.trim().length > 0;
    if (isStreaming && hasContent && !autoOpenedRef.current) {
      setExpanded(true);
      autoOpenedRef.current = true;
    }
    if (!isStreaming) {
      autoOpenedRef.current = false;
    }
  }, [content, isStreaming]);

  // Track elapsed thinking time
  useEffect(() => {
    if (!isStreaming) {
      // Capture final elapsed
      setElapsed(Math.round((Date.now() - startTimeRef.current) / 1000));
      return;
    }

    const interval = setInterval(() => {
      setElapsed(Math.round((Date.now() - startTimeRef.current) / 1000));
    }, 1000);

    return () => clearInterval(interval);
  }, [isStreaming]);

  const tokenEstimate = Math.round(content.length / 4); // rough estimate
  const summaryExcerpt = !isStreaming
    ? content
        .replace(/[#>*`_~-]/g, ' ')
        .replace(/\s+/g, ' ')
        .trim()
        .slice(0, 88)
    : '';

  const summaryText = isStreaming
    ? t('chat.thinkingElapsed', { seconds: elapsed.toString() })
    : elapsed > 0
      ? t('chat.thoughtFor', { seconds: elapsed.toString() })
      : t('chat.thinkingCompleted');

  return (
    <div className="mb-2">
      <button
        type="button"
        onClick={() => setExpanded(!expanded)}
        className="flex items-center gap-1.5 text-xs text-text-tertiary hover:text-text-secondary transition-colors cursor-pointer group"
      >
        <ChevronRight
          size={12}
          className={`transition-transform duration-200 ${expanded ? 'rotate-90' : ''}`}
        />
        <span className="flex items-center gap-1.5">
          <Brain size={12} />
          <span>{summaryText}</span>
          {isStreaming && (
            <span className="flex gap-0.5 ml-0.5">
              <span className="w-1 h-1 rounded-full bg-text-tertiary animate-bounce" style={{ animationDelay: '0ms' }} />
              <span className="w-1 h-1 rounded-full bg-text-tertiary animate-bounce" style={{ animationDelay: '150ms' }} />
              <span className="w-1 h-1 rounded-full bg-text-tertiary animate-bounce" style={{ animationDelay: '300ms' }} />
            </span>
          )}
          {!isStreaming && tokenEstimate > 0 && (
            <span className="text-text-tertiary/60">. {t('chat.tokenEstimate', { count: tokenEstimate.toString() })}</span>
          )}
          {!isStreaming && summaryExcerpt && (
            <span className="max-w-[28rem] truncate text-text-tertiary/70">. {summaryExcerpt}</span>
          )}
        </span>
      </button>

      <AnimatePresence initial={false}>
        {expanded && content && (
          <motion.div
            initial={{ height: 0, opacity: 0 }}
            animate={{ height: 'auto', opacity: 1 }}
            exit={{ height: 0, opacity: 0 }}
            transition={{ duration: 0.2, ease: [0.16, 1, 0.3, 1] }}
            className="overflow-hidden"
          >
            <div className="mt-1.5 ml-4 pl-3 border-l-2 border-text-tertiary/20 bg-surface-0/40 rounded-r-md py-2 px-3 text-xs text-text-secondary leading-relaxed max-h-[300px] overflow-y-auto">
              <ReactMarkdown remarkPlugins={[remarkGfm]} components={thinkingMarkdownComponents}>
                {content}
              </ReactMarkdown>
              {isStreaming && (
                <span className="inline-block w-1.5 h-3 bg-text-tertiary/50 animate-pulse ml-0.5 align-text-bottom rounded-sm" />
              )}
            </div>
          </motion.div>
        )}
      </AnimatePresence>
    </div>
  );
}
