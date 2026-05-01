import {
  useCallback,
  useEffect,
  useMemo,
  useRef,
  useState,
  type ReactNode,
} from 'react';
import { AnimatePresence, motion, useReducedMotion } from 'framer-motion';
import ReactMarkdown from 'react-markdown';
import remarkGfm from 'remark-gfm';
import { toast } from 'sonner';
import {
  Check,
  Copy,
  ExternalLink,
  Eye,
  FileCode2,
  FileText,
  FolderOpen,
  Loader2,
  PanelRightClose,
  RotateCcw,
  Save,
  SplitSquareHorizontal,
  SquarePen,
  TriangleAlert,
  X,
} from 'lucide-react';
import { useTranslation } from '../i18n';
import * as api from './api';
import { markdownComponents, rehypePlugins } from '../components/chat/markdownComponents';
import { FilePreviewContext } from './filePreviewContext';

type PreviewMode = 'preview' | 'edit' | 'split';

const INSTANT_TRANSITION = { duration: 0 };
const REMARK_PLUGINS = [remarkGfm];

function basename(path: string): string {
  const normalized = path.replace(/[\\/]+$/, '');
  const lastSep = Math.max(normalized.lastIndexOf('/'), normalized.lastIndexOf('\\'));
  return lastSep === -1 ? normalized : normalized.slice(lastSep + 1);
}

function formatBytes(bytes: number): string {
  if (!Number.isFinite(bytes) || bytes < 0) return '';
  if (bytes < 1024) return `${bytes} B`;
  const units = ['KB', 'MB', 'GB'];
  let value = bytes / 1024;
  for (const unit of units) {
    if (value < 1024 || unit === 'GB') {
      return `${value.toFixed(value < 10 ? 1 : 0)} ${unit}`;
    }
    value /= 1024;
  }
  return `${bytes} B`;
}

function formatTimestamp(value: string | null | undefined, locale: string): string {
  if (!value) return '';
  const date = new Date(value);
  if (Number.isNaN(date.getTime())) return '';
  return new Intl.DateTimeFormat(locale, {
    month: 'short',
    day: '2-digit',
    hour: '2-digit',
    minute: '2-digit',
  }).format(date);
}

function copyForLocale(locale: string) {
  const zh = locale.startsWith('zh');
  return {
    title: zh ? '文件预览' : 'File Preview',
    preview: zh ? '预览' : 'Preview',
    edit: zh ? '编辑' : 'Edit',
    split: zh ? '分屏' : 'Split',
    extracted: zh ? '提取文本' : 'Extracted Text',
    readOnly: zh ? '只读' : 'Read-only',
    editable: zh ? '可编辑' : 'Editable',
    save: zh ? '保存' : 'Save',
    saved: zh ? '已保存' : 'Saved',
    discard: zh ? '还原草稿' : 'Discard draft',
    reload: zh ? '重新加载' : 'Reload',
    openExternal: zh ? '外部打开' : 'Open externally',
    showFolder: zh ? '所在文件夹' : 'Show in folder',
    copyPath: zh ? '复制路径' : 'Copy path',
    copied: zh ? '已复制' : 'Copied',
    close: zh ? '关闭' : 'Close',
    loading: zh ? '正在读取文件...' : 'Reading file...',
    empty: zh ? '没有可预览的文本内容。' : 'No text content is available for preview.',
    unsupported: zh ? '这个文件暂时不能在应用内预览或编辑。' : 'This file cannot be previewed or edited inline yet.',
    conflict: zh ? '文件已在磁盘上变化，请重新加载后再保存。' : 'The file changed on disk. Reload before saving.',
    saveFailed: zh ? '保存失败' : 'Save failed',
    loadFailed: zh ? '预览失败' : 'Preview failed',
    reindexFailed: zh ? '文件已保存，但重新索引失败' : 'Saved, but reindexing failed',
    dirty: zh ? '未保存' : 'Unsaved',
    lines: zh ? '行' : 'lines',
    source: zh ? '来源' : 'Source',
    encoding: zh ? '编码' : 'Encoding',
    discardPrompt: zh ? '当前文件有未保存修改，确定要关闭吗？' : 'This file has unsaved changes. Close anyway?',
  };
}

function TextPreview({ content }: { content: string }) {
  const lines = content.split('\n');
  return (
    <pre className="min-h-full overflow-auto px-4 py-3 text-xs leading-5 text-text-secondary">
      {lines.map((line, index) => (
        <div key={index} className="grid grid-cols-[3rem_minmax(0,1fr)] gap-3">
          <span className="select-none text-right text-text-tertiary/70">{index + 1}</span>
          <code className="whitespace-pre-wrap break-words font-mono">{line || ' '}</code>
        </div>
      ))}
    </pre>
  );
}

function MarkdownPreview({ content }: { content: string }) {
  return (
    <div className="prose prose-sm prose-invert max-w-none px-5 py-4 text-text-primary">
      <ReactMarkdown
        remarkPlugins={REMARK_PLUGINS}
        rehypePlugins={rehypePlugins}
        components={markdownComponents}
      >
        {content}
      </ReactMarkdown>
    </div>
  );
}

function ModeButton({
  active,
  icon,
  label,
  onClick,
}: {
  active: boolean;
  icon: ReactNode;
  label: string;
  onClick: () => void;
}) {
  return (
    <button
      type="button"
      onClick={onClick}
      className={`inline-flex h-8 items-center gap-1.5 rounded-md px-2.5 text-xs font-medium transition-colors ${
        active
          ? 'bg-accent text-white'
          : 'text-text-secondary hover:bg-surface-3 hover:text-text-primary'
      }`}
    >
      {icon}
      <span>{label}</span>
    </button>
  );
}

export function FilePreviewProvider({ children }: { children: ReactNode }) {
  const { locale } = useTranslation();
  const labels = useMemo(() => copyForLocale(locale), [locale]);
  const shouldReduceMotion = useReducedMotion();
  const [open, setOpen] = useState(false);
  const [activePath, setActivePath] = useState<string | null>(null);
  const [preview, setPreview] = useState<api.FilePreview | null>(null);
  const [draft, setDraft] = useState('');
  const [mode, setMode] = useState<PreviewMode>('preview');
  const [loading, setLoading] = useState(false);
  const [saving, setSaving] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [copiedPath, setCopiedPath] = useState(false);
  const dirty = Boolean(preview?.editable && draft !== (preview.content ?? ''));
  const dirtyRef = useRef(false);

  useEffect(() => {
    dirtyRef.current = dirty;
  }, [dirty]);

  const loadFile = useCallback(async (path: string) => {
    setLoading(true);
    setError(null);
    setActivePath(path);
    try {
      const next = await api.previewFile(path);
      setPreview(next);
      setDraft(next.content ?? '');
      setMode(next.kind === 'markdown' ? 'preview' : next.editable ? 'edit' : 'preview');
      setActivePath(next.path);
    } catch (err) {
      const message = err instanceof Error ? err.message : String(err);
      setPreview(null);
      setDraft('');
      setError(message);
      toast.error(`${labels.loadFailed}: ${message}`);
    } finally {
      setLoading(false);
    }
  }, [labels.loadFailed]);

  const openFilePreview = useCallback((path: string) => {
    if (dirtyRef.current && !window.confirm(labels.discardPrompt)) {
      return;
    }
    setOpen(true);
    void loadFile(path);
  }, [labels.discardPrompt, loadFile]);

  const close = useCallback(() => {
    if (dirty && !window.confirm(labels.discardPrompt)) {
      return;
    }
    setOpen(false);
  }, [dirty, labels.discardPrompt]);

  const save = useCallback(async () => {
    if (!preview?.editable || !dirty) return;
    setSaving(true);
    setError(null);
    try {
      const result = await api.saveTextFile(preview.path, draft, preview.hash);
      setPreview(result.preview);
      setDraft(result.preview.content ?? '');
      toast.success(labels.saved);
      if (result.reindexStatus !== 'ok') {
        toast.warning(`${labels.reindexFailed}: ${result.reindexDetail ?? ''}`);
      }
    } catch (err) {
      const message = err instanceof Error ? err.message : String(err);
      setError(message);
      toast.error(`${labels.saveFailed}: ${message}`);
    } finally {
      setSaving(false);
    }
  }, [dirty, draft, labels.reindexFailed, labels.saveFailed, labels.saved, preview]);

  useEffect(() => {
    if (!open) return;
    const handler = (event: KeyboardEvent) => {
      if ((event.ctrlKey || event.metaKey) && event.key.toLowerCase() === 's') {
        event.preventDefault();
        void save();
      }
      if (event.key === 'Escape') {
        close();
      }
    };
    window.addEventListener('keydown', handler);
    return () => window.removeEventListener('keydown', handler);
  }, [close, open, save]);

  const contextValue = useMemo(() => ({ openFilePreview }), [openFilePreview]);
  const content = preview?.content ?? '';
  const canShowPreview = Boolean(preview?.content);
  const metadataBits = preview
    ? [
        formatBytes(preview.sizeBytes),
        preview.lineCount > 0 ? `${preview.lineCount} ${labels.lines}` : '',
        formatTimestamp(preview.modifiedAt, locale),
        preview.encoding ? `${labels.encoding}: ${preview.encoding}` : '',
      ].filter(Boolean)
    : [];

  return (
    <FilePreviewContext.Provider value={contextValue}>
      {children}
      <AnimatePresence>
        {open && (
          <motion.aside
            initial={shouldReduceMotion ? false : { x: '100%', opacity: 0.8 }}
            animate={{ x: 0, opacity: 1 }}
            exit={shouldReduceMotion ? { opacity: 0 } : { x: '100%', opacity: 0.8 }}
            transition={shouldReduceMotion ? INSTANT_TRANSITION : { duration: 0.24, ease: [0.16, 1, 0.3, 1] }}
            className="fixed inset-y-0 right-0 z-50 flex w-full max-w-[min(760px,100vw)] flex-col border-l border-border bg-surface-1 shadow-2xl md:w-[46vw] md:min-w-[420px]"
            aria-label={labels.title}
          >
            <header className="shrink-0 border-b border-border bg-surface-1/95 px-4 py-3 backdrop-blur">
              <div className="flex items-start gap-3">
                <div className="mt-0.5 flex h-9 w-9 shrink-0 items-center justify-center rounded-md border border-border bg-surface-2 text-accent">
                  {preview?.kind === 'code' ? <FileCode2 size={18} /> : <FileText size={18} />}
                </div>
                <div className="min-w-0 flex-1">
                  <div className="flex min-w-0 items-center gap-2">
                    <h2 className="truncate text-sm font-semibold text-text-primary">
                      {preview?.displayName ?? basename(activePath ?? labels.title)}
                    </h2>
                    {dirty && (
                      <span className="shrink-0 rounded-full border border-warning/30 bg-warning/10 px-2 py-0.5 text-[10px] font-medium text-warning">
                        {labels.dirty}
                      </span>
                    )}
                    {preview && (
                      <span className={`shrink-0 rounded-full border px-2 py-0.5 text-[10px] font-medium ${
                        preview.editable
                          ? 'border-success/20 bg-success/10 text-success'
                          : 'border-border bg-surface-2 text-text-tertiary'
                      }`}>
                        {preview.editable ? labels.editable : labels.readOnly}
                      </span>
                    )}
                  </div>
                  <p className="mt-1 truncate text-[11px] text-text-tertiary" title={preview?.path ?? activePath ?? ''}>
                    {preview?.path ?? activePath}
                  </p>
                  {preview && (
                    <p className="mt-1 truncate text-[11px] text-text-tertiary">
                      {labels.source}: {preview.sourceName}
                      {metadataBits.length > 0 ? ` · ${metadataBits.join(' · ')}` : ''}
                    </p>
                  )}
                </div>
                <button
                  type="button"
                  onClick={close}
                  className="rounded-md p-2 text-text-tertiary transition-colors hover:bg-surface-2 hover:text-text-primary"
                  title={labels.close}
                  aria-label={labels.close}
                >
                  <PanelRightClose size={18} />
                </button>
              </div>

              <div className="mt-3 flex flex-wrap items-center gap-2">
                <div className="flex rounded-md border border-border bg-surface-2 p-0.5">
                  <ModeButton
                    active={mode === 'preview'}
                    icon={<Eye size={14} />}
                    label={preview?.kind === 'document' ? labels.extracted : labels.preview}
                    onClick={() => setMode('preview')}
                  />
                  {preview?.editable && (
                    <>
                      <ModeButton
                        active={mode === 'edit'}
                        icon={<SquarePen size={14} />}
                        label={labels.edit}
                        onClick={() => setMode('edit')}
                      />
                      {preview.kind === 'markdown' && (
                        <ModeButton
                          active={mode === 'split'}
                          icon={<SplitSquareHorizontal size={14} />}
                          label={labels.split}
                          onClick={() => setMode('split')}
                        />
                      )}
                    </>
                  )}
                </div>

                <div className="flex-1" />

                {preview?.editable && (
                  <>
                    <button
                      type="button"
                      disabled={!dirty || saving}
                      onClick={() => setDraft(preview.content ?? '')}
                      className="inline-flex h-8 items-center gap-1.5 rounded-md px-2.5 text-xs font-medium text-text-secondary transition-colors hover:bg-surface-2 hover:text-text-primary disabled:pointer-events-none disabled:opacity-40"
                    >
                      <RotateCcw size={14} />
                      {labels.discard}
                    </button>
                    <button
                      type="button"
                      disabled={!dirty || saving}
                      onClick={save}
                      className="inline-flex h-8 items-center gap-1.5 rounded-md bg-accent px-3 text-xs font-medium text-white transition-colors hover:bg-accent-hover disabled:pointer-events-none disabled:opacity-40"
                    >
                      {saving ? <Loader2 size={14} className="animate-spin" /> : <Save size={14} />}
                      {labels.save}
                    </button>
                  </>
                )}

                {preview && (
                  <>
                    <button
                      type="button"
                      onClick={() => {
                        void api.openFileInDefaultApp(preview.path);
                      }}
                      className="inline-flex h-8 items-center justify-center rounded-md px-2 text-text-tertiary transition-colors hover:bg-surface-2 hover:text-text-primary"
                      title={labels.openExternal}
                      aria-label={labels.openExternal}
                    >
                      <ExternalLink size={15} />
                    </button>
                    <button
                      type="button"
                      onClick={() => {
                        void api.showInFileExplorer(preview.path);
                      }}
                      className="inline-flex h-8 items-center justify-center rounded-md px-2 text-text-tertiary transition-colors hover:bg-surface-2 hover:text-text-primary"
                      title={labels.showFolder}
                      aria-label={labels.showFolder}
                    >
                      <FolderOpen size={15} />
                    </button>
                    <button
                      type="button"
                      onClick={async () => {
                        await navigator.clipboard.writeText(preview.path);
                        setCopiedPath(true);
                        setTimeout(() => setCopiedPath(false), 1600);
                      }}
                      className="inline-flex h-8 items-center justify-center rounded-md px-2 text-text-tertiary transition-colors hover:bg-surface-2 hover:text-text-primary"
                      title={labels.copyPath}
                      aria-label={labels.copyPath}
                    >
                      {copiedPath ? <Check size={15} className="text-success" /> : <Copy size={15} />}
                    </button>
                    <button
                      type="button"
                      onClick={() => {
                        if (preview) void loadFile(preview.path);
                      }}
                      className="inline-flex h-8 items-center justify-center rounded-md px-2 text-text-tertiary transition-colors hover:bg-surface-2 hover:text-text-primary"
                      title={labels.reload}
                      aria-label={labels.reload}
                    >
                      <RotateCcw size={15} />
                    </button>
                  </>
                )}
              </div>
            </header>

            {(preview?.warning || error) && (
              <div className="shrink-0 border-b border-warning/20 bg-warning/10 px-4 py-2">
                <div className="flex items-start gap-2 text-xs text-warning">
                  <TriangleAlert size={14} className="mt-0.5 shrink-0" />
                  <p className="min-w-0 whitespace-pre-wrap break-words">{error ?? preview?.warning}</p>
                </div>
              </div>
            )}

            <div className="min-h-0 flex-1 overflow-hidden bg-surface-0">
              {loading ? (
                <div className="flex h-full items-center justify-center gap-2 text-sm text-text-tertiary">
                  <Loader2 size={16} className="animate-spin" />
                  {labels.loading}
                </div>
              ) : !preview ? (
                <div className="flex h-full flex-col items-center justify-center gap-3 px-6 text-center text-sm text-text-tertiary">
                  <FileText size={28} />
                  <p>{error ?? labels.unsupported}</p>
                  <button
                    type="button"
                    onClick={close}
                    className="inline-flex h-8 items-center gap-1.5 rounded-md px-3 text-xs font-medium text-text-secondary transition-colors hover:bg-surface-2 hover:text-text-primary"
                  >
                    <X size={14} />
                    {labels.close}
                  </button>
                </div>
              ) : mode === 'edit' && preview.editable ? (
                <textarea
                  value={draft}
                  onChange={(event) => setDraft(event.target.value)}
                  spellCheck={false}
                  className="h-full w-full resize-none border-0 bg-surface-0 px-4 py-3 font-mono text-xs leading-5 text-text-primary outline-none placeholder:text-text-tertiary"
                />
              ) : mode === 'split' && preview.editable && preview.kind === 'markdown' ? (
                <div className="grid h-full grid-cols-1 md:grid-cols-2">
                  <textarea
                    value={draft}
                    onChange={(event) => setDraft(event.target.value)}
                    spellCheck={false}
                    className="h-full w-full resize-none border-0 border-r border-border bg-surface-0 px-4 py-3 font-mono text-xs leading-5 text-text-primary outline-none placeholder:text-text-tertiary md:border-r"
                  />
                  <div className="h-full overflow-auto bg-surface-1">
                    <MarkdownPreview content={draft} />
                  </div>
                </div>
              ) : canShowPreview ? (
                <div className="h-full overflow-auto">
                  {preview.kind === 'markdown' ? (
                    <MarkdownPreview content={preview.editable ? draft : content} />
                  ) : (
                    <TextPreview content={preview.editable ? draft : content} />
                  )}
                </div>
              ) : (
                <div className="flex h-full items-center justify-center px-6 text-center text-sm text-text-tertiary">
                  {preview.kind === 'binary' ? labels.unsupported : labels.empty}
                </div>
              )}
            </div>
          </motion.aside>
        )}
      </AnimatePresence>
    </FilePreviewContext.Provider>
  );
}
