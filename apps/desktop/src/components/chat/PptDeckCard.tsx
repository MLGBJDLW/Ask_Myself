import { useEffect, useRef, useState } from 'react';
import { renderDeck } from '../../lib/ppt';
import type { DeckSpec } from '../../lib/ppt';
import { openFileInDefaultApp, savePptxBytes, showInFileExplorer } from '../../lib/api';
import { save as saveDialog } from '@tauri-apps/plugin-dialog';

type RenderState =
  | { kind: 'idle' }
  | { kind: 'rendering' }
  | { kind: 'saved'; path: string }
  | { kind: 'error'; message: string };

export interface PptDeckCardProps {
  artifactKey: string;
  path: string;
  spec: DeckSpec;
}

export function PptDeckCard({ artifactKey, path, spec }: PptDeckCardProps) {
  const [state, setState] = useState<RenderState>({ kind: 'idle' });
  const autoRunRef = useRef<Set<string>>(new Set());

  useEffect(() => {
    if (autoRunRef.current.has(artifactKey)) return;
    autoRunRef.current.add(artifactKey);
    void runAndSave(path);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [artifactKey]);

  async function runAndSave(targetPath: string) {
    setState({ kind: 'rendering' });
    try {
      const bytes = await renderDeck(spec);
      const saved = await savePptxBytes(targetPath, bytes);
      setState({ kind: 'saved', path: saved });
    } catch (err) {
      const msg = err instanceof Error ? err.message : String(err);
      setState({ kind: 'error', message: msg });
    }
  }

  async function handleSaveAs() {
    try {
      const chosen = await saveDialog({
        defaultPath: path,
        filters: [{ name: 'PowerPoint', extensions: ['pptx'] }],
      });
      if (!chosen) return;
      await runAndSave(chosen);
    } catch (err) {
      const msg = err instanceof Error ? err.message : String(err);
      setState({ kind: 'error', message: `Save dialog failed: ${msg}` });
    }
  }

  const theme =
    typeof spec.theme === 'string'
      ? spec.theme
      : spec.theme
        ? 'custom'
        : 'nexa-light';
  const firstSlide = spec.slides[0];
  const firstSlideLabel = firstSlide
    ? 'title' in firstSlide && firstSlide.title
      ? firstSlide.title
      : firstSlide.layout
    : '(empty)';

  return (
    <div className="rounded-md border border-border bg-surface-1 p-3 my-2 text-sm">
      <div className="flex items-center justify-between gap-3 mb-2">
        <div className="flex items-center gap-2 font-medium">
          <span aria-hidden>📊</span>
          <span className="truncate text-text-primary">{spec.title || 'Untitled deck'}</span>
        </div>
        <span className="text-xs text-text-tertiary">
          {spec.slides.length} slide{spec.slides.length === 1 ? '' : 's'} · {theme}
        </span>
      </div>
      <div className="text-xs text-text-tertiary truncate mb-2">
        First slide: {firstSlideLabel}
      </div>

      {state.kind === 'idle' && (
        <div className="text-xs text-text-tertiary">Preparing renderer…</div>
      )}

      {state.kind === 'rendering' && (
        <div className="flex items-center gap-2 text-xs text-text-tertiary">
          <span className="inline-block h-2 w-2 rounded-full bg-accent animate-pulse" />
          Rendering slides…
        </div>
      )}

      {state.kind === 'saved' && (
        <div className="flex flex-wrap items-center gap-2">
          <span className="text-xs text-success">Saved:</span>
          <code className="text-xs px-1 py-0.5 rounded bg-surface-2 truncate max-w-[60ch] text-text-secondary">
            {state.path}
          </code>
          <button
            type="button"
            onClick={() => void openFileInDefaultApp(state.path)}
            className="text-xs px-2 py-1 rounded bg-accent text-text-inverse hover:bg-accent-hover cursor-pointer"
          >
            Open
          </button>
          <button
            type="button"
            onClick={() => void showInFileExplorer(state.path)}
            className="text-xs px-2 py-1 rounded border border-border text-text-secondary hover:bg-surface-2 cursor-pointer"
          >
            Reveal
          </button>
        </div>
      )}

      {state.kind === 'error' && (
        <div className="space-y-2">
          <div className="text-xs text-danger">Error: {state.message}</div>
          <button
            type="button"
            onClick={() => void handleSaveAs()}
            className="text-xs px-2 py-1 rounded bg-accent text-text-inverse hover:bg-accent-hover cursor-pointer"
          >
            Save as…
          </button>
        </div>
      )}
    </div>
  );
}
