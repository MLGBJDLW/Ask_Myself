import {
  useCallback,
  useEffect,
  useMemo,
  useRef,
  useState,
  type KeyboardEvent as ReactKeyboardEvent,
  type UIEvent,
} from 'react';
import { AlertTriangle, Eye, FileText, Pencil, Save, SplitSquareHorizontal, Upload, X } from 'lucide-react';
import { Button } from '../ui/Button';
import { Input } from '../ui/Input';
import { useTranslation } from '../../i18n';
import * as api from '../../lib/api';
import type { SaveSkillInput, Skill, SkillWarning } from '../../types/extensions';
import { SkillMarkdownPreview } from './SkillMarkdownPreview';

interface SkillEditorProps {
  skill?: Skill;
  onSave: (input: SaveSkillInput) => void;
  onCancel: () => void;
  onDirtyChange?: (dirty: boolean) => void;
}

type Mode = 'edit' | 'preview' | 'split';

const DESCRIPTION_MAX = 500;

function estimateTokens(text: string): number {
  if (!text) return 0;
  let tokens = 0;
  for (let i = 0; i < text.length; i++) {
    tokens += text.charCodeAt(i) > 0x2fff ? 1.5 : 0.25;
  }
  return Math.ceil(tokens);
}

/** Detects a leading YAML frontmatter block (`^---\n … \n---\n`) and returns
 *  its character range in the textarea so we can highlight it. */
function detectFrontmatterRange(
  content: string,
): { start: number; end: number } | null {
  const text = content.replace(/^\uFEFF/, '');
  const match = text.match(/^---\r?\n([\s\S]*?)\r?\n---\r?\n?/);
  if (!match) return null;
  return { start: 0, end: match[0].length };
}

export function SkillEditor({ skill, onSave, onCancel, onDirtyChange }: SkillEditorProps) {
  const { t } = useTranslation();
  const [name, setName] = useState(skill?.name ?? '');
  const [description, setDescription] = useState(skill?.description ?? '');
  const [content, setContent] = useState(skill?.content ?? '');
  const [importError, setImportError] = useState<string | null>(null);
  const [pendingImport, setPendingImport] = useState<
    | { text: string; warnings: SkillWarning[] }
    | null
  >(null);
  const [mode, setMode] = useState<Mode>('edit');
  const fileInputRef = useRef<HTMLInputElement | null>(null);
  const textareaRef = useRef<HTMLTextAreaElement | null>(null);
  const lineNumbersRef = useRef<HTMLDivElement | null>(null);
  const initialDraftRef = useRef({
    name: skill?.name ?? '',
    description: skill?.description ?? '',
    content: skill?.content ?? '',
  });

  useEffect(() => {
    if (!onDirtyChange) return;
    const dirty =
      name !== initialDraftRef.current.name ||
      description !== initialDraftRef.current.description ||
      content !== initialDraftRef.current.content;
    onDirtyChange(dirty);
  }, [content, description, name, onDirtyChange]);

  useEffect(() => {
    if (!onDirtyChange) return;
    return () => {
      onDirtyChange(false);
    };
  }, [onDirtyChange]);

  const handleSubmit = useCallback(() => {
    if (!name.trim() || !content.trim()) return;
    onSave({
      id: skill?.id ?? null,
      name: name.trim(),
      description: description.trim().slice(0, DESCRIPTION_MAX),
      content: content.trim(),
      enabled: skill?.enabled ?? true,
    });
  }, [content, description, name, onSave, skill?.enabled, skill?.id]);

  // Ctrl/Cmd+S to save; Tab -> two spaces so the textarea behaves like a source editor.
  const handleTextareaKeyDown = (e: ReactKeyboardEvent<HTMLTextAreaElement>) => {
    if ((e.ctrlKey || e.metaKey) && e.key.toLowerCase() === 's') {
      e.preventDefault();
      handleSubmit();
      return;
    }
    if (e.key === 'Tab') {
      e.preventDefault();
      const ta = e.currentTarget;
      const start = ta.selectionStart;
      const end = ta.selectionEnd;
      const next = content.slice(0, start) + '  ' + content.slice(end);
      setContent(next);
      requestAnimationFrame(() => {
        ta.selectionStart = ta.selectionEnd = start + 2;
      });
    }
  };

  const tokenCount = estimateTokens(content);
  const lineCount = content ? content.split('\n').length : 1;
  const charCount = content.length;

  const lineNumbers = useMemo(() => {
    const arr: number[] = [];
    for (let i = 1; i <= lineCount; i++) arr.push(i);
    return arr;
  }, [lineCount]);

  const frontmatterRange = useMemo(() => detectFrontmatterRange(content), [content]);

  const handleTextareaScroll = (e: UIEvent<HTMLTextAreaElement>) => {
    const gutter = lineNumbersRef.current;
    if (gutter) gutter.scrollTop = e.currentTarget.scrollTop;
  };

  const SKILL_TEMPLATE = `## Trigger
[When should this skill activate?]

## Rules
1. [Rule 1]
2. [Rule 2]

## Example
[Before/after showing the quality difference]`;

  const handleUseTemplate = () => {
    setContent(SKILL_TEMPLATE);
  };

  const handleImportClick = () => {
    setImportError(null);
    fileInputRef.current?.click();
  };

  const runImport = useCallback(async (text: string) => {
    try {
      const imported = await api.importSkillFromMd(text);
      setName(imported.name);
      setDescription(imported.description);
      setContent(imported.content);
    } catch (err) {
      const message = err instanceof Error ? err.message : String(err);
      setImportError(t('settings.skillImportError', { message }));
    }
  }, [t]);

  const handleFileSelected = async (e: React.ChangeEvent<HTMLInputElement>) => {
    const file = e.target.files?.[0];
    e.target.value = '';
    if (!file) return;
    try {
      const text = await file.text();
      let warnings: SkillWarning[] = [];
      try {
        warnings = await api.scanSkillContent(text);
      } catch {
        // Scanner failure is non-fatal; fall back to direct import.
      }
      if (warnings.length > 0) {
        setPendingImport({ text, warnings });
        return;
      }
      await runImport(text);
    } catch (err) {
      const message = err instanceof Error ? err.message : String(err);
      setImportError(t('settings.skillImportError', { message }));
    }
  };

  const tabs: Array<{ id: Mode; label: string; icon: React.ReactNode }> = [
    { id: 'edit', label: t('settings.skillTabEdit'), icon: <Pencil size={12} /> },
    { id: 'preview', label: t('settings.skillTabPreview'), icon: <Eye size={12} /> },
    { id: 'split', label: t('settings.skillTabSplit'), icon: <SplitSquareHorizontal size={12} /> },
  ];

  // Preview needs synthesised frontmatter when body is plain markdown.
  const previewDoc = useMemo(() => {
    if (/^---\r?\n/.test(content)) return content;
    const nm = name || 'untitled';
    const desc = description || '';
    const escape = (v: string) =>
      /[:#\n"]/.test(v) ? `"${v.replace(/\\/g, '\\\\').replace(/"/g, '\\"').replace(/\n/g, ' ')}"` : v;
    return `---\nname: ${escape(nm)}\ndescription: ${escape(desc)}\n---\n\n${content}`;
  }, [content, description, name]);

  const showEditor = mode === 'edit' || mode === 'split';
  const showPreview = mode === 'preview' || mode === 'split';

  return (
    <div className="space-y-4">
      <input
        ref={fileInputRef}
        type="file"
        accept=".md,text/markdown"
        className="hidden"
        onChange={handleFileSelected}
      />

      <div className="flex flex-wrap items-center justify-between gap-2">
        <div className="inline-flex rounded-md border border-border bg-surface-2 p-0.5">
          {tabs.map((tab) => (
            <button
              key={tab.id}
              type="button"
              onClick={() => setMode(tab.id)}
              className={`inline-flex items-center gap-1 rounded px-2.5 py-1 text-xs transition-colors ${
                mode === tab.id
                  ? 'bg-accent text-white'
                  : 'text-text-secondary hover:text-text-primary'
              }`}
            >
              {tab.icon}
              {tab.label}
            </button>
          ))}
        </div>
        {!skill && (
          <Button variant="ghost" size="sm" icon={<Upload size={14} />} onClick={handleImportClick}>
            {t('settings.skillImportMd')}
          </Button>
        )}
      </div>

      {importError && (
        <div className="rounded-md border border-danger/40 bg-danger/10 px-3 py-2 text-xs text-danger">
          {importError}
        </div>
      )}

      {pendingImport && (
        <ImportScanWarning
          warnings={pendingImport.warnings}
          onCancel={() => setPendingImport(null)}
          onConfirm={async () => {
            const { text } = pendingImport;
            setPendingImport(null);
            await runImport(text);
          }}
        />
      )}

      <div className="space-y-2">
        <label className="text-sm font-medium text-text-primary">{t('settings.skillName')}</label>
        <Input
          value={name}
          onChange={(e) => setName(e.target.value)}
          placeholder={t('settings.skillName')}
        />
      </div>

      <div className="space-y-2">
        <label className="text-sm font-medium text-text-primary">
          {t('settings.skillDescriptionLabel')}
        </label>
        <textarea
          value={description}
          onChange={(e) => setDescription(e.target.value.slice(0, DESCRIPTION_MAX))}
          placeholder={t('settings.skillDescriptionPlaceholder')}
          rows={2}
          className="w-full rounded-md border border-border bg-surface-2 px-3 py-2 text-sm text-text-primary placeholder:text-text-tertiary focus:border-accent focus:outline-none focus:ring-1 focus:ring-accent resize-y"
        />
        <p className="text-xs text-text-tertiary">{description.length}/{DESCRIPTION_MAX}</p>
      </div>

      <div className="space-y-2">
        <div className="flex items-center justify-between">
          <label className="text-sm font-medium text-text-primary">{t('settings.skillContent')}</label>
          {!content.trim() && mode !== 'preview' && (
            <Button variant="ghost" size="sm" icon={<FileText size={14} />} onClick={handleUseTemplate}>
              {t('settings.skillUseTemplate')}
            </Button>
          )}
        </div>

        <div
          className={`grid gap-3 ${mode === 'split' ? 'grid-cols-1 lg:grid-cols-2' : 'grid-cols-1'}`}
        >
          {showEditor && (
            <div className="relative">
              {frontmatterRange && (
                <FrontmatterBand content={content} range={frontmatterRange} />
              )}
              <div className="relative flex min-h-[220px] overflow-hidden rounded-md border border-border bg-surface-2 focus-within:border-accent focus-within:ring-1 focus-within:ring-accent">
                <div
                  ref={lineNumbersRef}
                  aria-hidden
                  className="select-none overflow-hidden bg-surface-3/50 px-2 py-2 text-right font-mono text-[11px] leading-5 text-text-tertiary"
                  style={{ minWidth: '2.5rem' }}
                >
                  {lineNumbers.map((n) => (
                    <div key={n} style={{ height: '1.25rem' }}>
                      {n}
                    </div>
                  ))}
                </div>
                <textarea
                  ref={textareaRef}
                  value={content}
                  onChange={(e) => setContent(e.target.value)}
                  onScroll={handleTextareaScroll}
                  onKeyDown={handleTextareaKeyDown}
                  placeholder={t('settings.skillContentPlaceholder')}
                  spellCheck={false}
                  className="relative z-10 flex-1 resize-none bg-transparent px-3 py-2 font-mono text-sm leading-5 text-text-primary placeholder:text-text-tertiary focus:outline-none"
                  style={{ minHeight: '220px', lineHeight: '1.25rem' }}
                  rows={12}
                />
              </div>
            </div>
          )}

          {showPreview && (
            <SkillMarkdownPreview
              content={previewDoc}
              fallbackName={name}
              fallbackDescription={description}
              className="max-h-[480px] overflow-auto"
            />
          )}
        </div>

        <p className="text-xs text-text-tertiary">
          {t('settings.skillStats', {
            lines: String(lineCount),
            chars: String(charCount),
            tokens: String(tokenCount),
          })}
        </p>
      </div>

      <div className="flex items-center gap-2 pt-2 border-t border-border">
        <Button
          variant="primary"
          size="sm"
          icon={<Save size={14} />}
          onClick={handleSubmit}
          disabled={!name.trim() || !content.trim()}
        >
          {t('common.save')}
        </Button>
        <Button variant="ghost" size="sm" icon={<X size={14} />} onClick={onCancel}>
          {t('common.cancel')}
        </Button>
        <span className="ml-auto text-[11px] text-text-tertiary">
          {t('settings.skillShortcutHint')}
        </span>
      </div>
    </div>
  );
}

function FrontmatterBand({
  content,
  range,
}: {
  content: string;
  range: { start: number; end: number };
}) {
  const linesBefore = content.slice(0, range.start).split('\n').length - 1;
  const spannedLines = content.slice(range.start, range.end).split('\n').length;
  const topRem = linesBefore * 1.25 + 0.5;
  const heightRem = spannedLines * 1.25;
  return (
    <div
      aria-hidden
      className="pointer-events-none absolute left-[2.5rem] right-0 z-0 rounded bg-accent/5 border-l-2 border-accent/40"
      style={{ top: `${topRem}rem`, height: `${heightRem}rem` }}
    />
  );
}

function ImportScanWarning({
  warnings,
  onCancel,
  onConfirm,
}: {
  warnings: SkillWarning[];
  onCancel: () => void;
  onConfirm: () => void;
}) {
  const { t } = useTranslation();
  const hasBlock = warnings.some((w) => w.severity === 'block');
  return (
    <div
      role="alert"
      className={`rounded-md border p-3 text-xs ${
        hasBlock
          ? 'border-danger/50 bg-danger/10'
          : 'border-warning/50 bg-warning/10'
      }`}
    >
      <div className="mb-2 flex items-center gap-2 font-medium">
        <AlertTriangle size={14} className={hasBlock ? 'text-danger' : 'text-warning'} />
        <span className={hasBlock ? 'text-danger' : 'text-warning'}>
          {t('settings.skillImportScanTitle')}
        </span>
      </div>
      <p className="mb-2 text-text-secondary">{t('settings.skillImportScanSubtitle')}</p>
      <ul className="mb-3 space-y-1">
        {warnings.map((w, i) => (
          <li key={`${w.code}-${i}`} className="flex items-start gap-2">
            <SeverityBadge severity={w.severity} />
            <span className="text-text-primary">
              <code className="mr-1 rounded bg-surface-3 px-1 py-0.5 text-[10px] text-text-secondary">
                {w.code}
              </code>
              {w.message}
            </span>
          </li>
        ))}
      </ul>
      <div className="flex items-center gap-2">
        <Button variant="ghost" size="sm" onClick={onCancel}>
          {t('common.cancel')}
        </Button>
        <Button
          variant={hasBlock ? 'danger' : 'primary'}
          size="sm"
          onClick={onConfirm}
        >
          {t('settings.skillImportProceed')}
        </Button>
      </div>
    </div>
  );
}

function SeverityBadge({ severity }: { severity: SkillWarning['severity'] }) {
  const cls =
    severity === 'block'
      ? 'bg-danger/20 text-danger border-danger/40'
      : severity === 'warn'
        ? 'bg-warning/20 text-warning border-warning/40'
        : 'bg-surface-3 text-text-secondary border-border';
  return (
    <span
      className={`mt-0.5 inline-flex min-w-[3rem] justify-center rounded border px-1 py-0.5 text-[10px] uppercase tracking-wide ${cls}`}
    >
      {severity}
    </span>
  );
}
