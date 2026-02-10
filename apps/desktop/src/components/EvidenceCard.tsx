import type { EvidenceCard as EvidenceCardType, Highlight } from "../types";

interface Props {
  card: EvidenceCardType;
  onSaveToPlaybook?: (chunkId: string) => void;
}

function renderHighlightedContent(content: string, highlights: Highlight[]) {
  if (highlights.length === 0) {
    return <span>{content}</span>;
  }

  const sorted = [...highlights].sort((a, b) => a.start - b.start);
  const parts: React.ReactNode[] = [];
  let cursor = 0;

  for (let i = 0; i < sorted.length; i++) {
    const h = sorted[i];
    if (h.start > cursor) {
      parts.push(<span key={`t-${i}`}>{content.slice(cursor, h.start)}</span>);
    }
    parts.push(
      <mark key={`h-${i}`} className="bg-yellow-500/30 text-yellow-200 rounded px-0.5">
        {content.slice(h.start, h.end)}
      </mark>,
    );
    cursor = h.end;
  }

  if (cursor < content.length) {
    parts.push(<span key="tail">{content.slice(cursor)}</span>);
  }

  return <>{parts}</>;
}

function scoreColor(score: number): string {
  if (score >= 3) return "text-green-400";
  if (score >= 1) return "text-yellow-400";
  return "text-gray-400";
}

export function EvidenceCardComponent({ card, onSaveToPlaybook }: Props) {
  return (
    <div className="rounded-lg border border-gray-700 bg-gray-800/60 p-4 transition hover:border-gray-600">
      {/* Header */}
      <div className="mb-2 flex items-start justify-between gap-2">
        <div className="min-w-0 flex-1">
          <p className="truncate text-xs text-gray-400" title={card.documentPath}>
            {card.documentPath}
          </p>
          {card.headingPath.length > 0 && (
            <p className="mt-0.5 truncate text-xs text-gray-500">
              {card.headingPath.join(" › ")}
            </p>
          )}
        </div>
        <span className={`shrink-0 text-xs font-mono font-semibold ${scoreColor(card.score)}`}>
          {card.score.toFixed(1)}
        </span>
      </div>

      {/* Content */}
      <div className="text-sm leading-relaxed text-gray-200 whitespace-pre-wrap">
        {renderHighlightedContent(card.content, card.highlights)}
      </div>

      {/* Footer */}
      <div className="mt-3 flex items-center justify-between">
        <span className="text-xs text-gray-500">{card.sourceName}</span>
        {onSaveToPlaybook && (
          <button
            onClick={() => onSaveToPlaybook(card.chunkId)}
            className="rounded px-2 py-1 text-xs text-blue-400 hover:bg-blue-500/10 hover:text-blue-300 transition"
          >
            + Save to Playbook
          </button>
        )}
      </div>
    </div>
  );
}
