import { memo, useMemo } from 'react';
import ReactMarkdown from 'react-markdown';
import { useStreamingPresentation } from '../../lib/useStreamingPresentation';

import { useTranslation } from '../../i18n';
import {
  preprocessChunkCitations,
  preprocessInlineCitations,
} from '../../lib/citationParser';
import type { CitationCardData } from '../../lib/citationParser';
import { buildEvidenceItemsFromContent } from '../../lib/evidenceItems';
import { MAX_HIGHLIGHT_DOCUMENT_CHARS } from '../../lib/streaming/markdownPresentation';
import { CitationChip } from './EvidenceCard';
import {
  CitationContext,
  MarkdownRenderStateProvider,
  markdownComponents,
  markdownRemarkPlugins,
  preprocessCitations,
  preprocessFilePaths,
  rehypePlugins,
} from './markdownComponents';

export interface MarkdownCitationLookup {
  getCard: (id: string) => CitationCardData | undefined;
}

const EMPTY_CITATION_LOOKUP: MarkdownCitationLookup = {
  getCard: () => undefined,
};

interface MarkdownDocumentProps {
  content: string;
  citationLookup: MarkdownCitationLookup;
  isStreaming: boolean;
}

const MarkdownDocument = memo(function MarkdownDocument({
  content,
  citationLookup,
  isStreaming,
}: MarkdownDocumentProps) {
  const { t } = useTranslation();
  const processed = useMemo(
    () => preprocessFilePaths(
      preprocessCitations(preprocessInlineCitations(preprocessChunkCitations(content))),
    ),
    [content],
  );
  const evidenceItems = useMemo(
    () => buildEvidenceItemsFromContent(
      content,
      citationLookup,
      (index) => t('chat.evidenceSourceLabel', { index: String(index) }),
    ),
    [citationLookup, content, t],
  );

  return (
    <>
      {evidenceItems.length > 0 && (
        <div className="mb-3 rounded-xl border border-border/70 bg-surface-1/70 px-2.5 py-2">
          <div className="mb-1 flex items-center justify-between gap-2">
            <span className="text-[11px] font-medium text-text-secondary">
              {t('chat.answerEvidence')}
            </span>
            <span className="text-[10px] text-text-tertiary">
              {t('chat.answerEvidenceSummary', { count: String(evidenceItems.length) })}
            </span>
          </div>
          <div className="flex flex-wrap gap-1.5">
            {evidenceItems.map((item) => (
              <CitationChip
                key={item.chunkId}
                chunkId={item.chunkId}
                displayText={item.displayText}
                card={item.card}
              />
            ))}
          </div>
        </div>
      )}
      <CitationContext.Provider value={citationLookup}>
        <MarkdownRenderStateProvider isStreaming={isStreaming} plainCode={content.length > MAX_HIGHLIGHT_DOCUMENT_CHARS}>
          <div
            className="prose-chat"
            data-testid="chat-markdown-document"
            data-markdown-source-chars={content.length}
          >
            <ReactMarkdown
              remarkPlugins={markdownRemarkPlugins}
              rehypePlugins={rehypePlugins}
              components={markdownComponents}
              urlTransform={(url) => url}
            >
              {processed}
            </ReactMarkdown>
          </div>
        </MarkdownRenderStateProvider>
      </CitationContext.Provider>
    </>
  );
});

export const StreamingMarkdown = memo(function StreamingMarkdown({
  content,
  isStreaming,
  citationLookup,
  reduceMotion,
}: {
  content: string;
  isStreaming: boolean;
  citationLookup?: MarkdownCitationLookup;
  reduceMotion: boolean;
}) {
  const presented = useStreamingPresentation(content, isStreaming, reduceMotion);
  const effectiveCitationLookup = citationLookup ?? EMPTY_CITATION_LOOKUP;

  return (
    <div className="relative">
      <MarkdownDocument
        content={presented}
        citationLookup={effectiveCitationLookup}
        isStreaming={isStreaming}
      />
      {isStreaming && (
        <span className={`streaming-caret-overlay ${reduceMotion ? '' : 'animate-pulse'}`} />
      )}
    </div>
  );
});
