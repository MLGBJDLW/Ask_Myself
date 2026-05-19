import {
  useCallback,
  useEffect,
  useMemo,
  useRef,
  useState,
  type CSSProperties,
  type KeyboardEvent as ReactKeyboardEvent,
  type ReactNode,
} from 'react';
import { useNavigate } from 'react-router-dom';
import { convertFileSrc } from '@tauri-apps/api/core';
import { AnimatePresence, motion, useReducedMotion } from 'framer-motion';
import ReactMarkdown from 'react-markdown';
import remarkGfm from 'remark-gfm';
import { toast } from 'sonner';
import {
  BotMessageSquare,
  Check,
  Copy,
  ExternalLink,
  Eye,
  FileCode2,
  FileSpreadsheet,
  FileText,
  FolderOpen,
  Image as ImageIcon,
  Languages,
  ListTree,
  Loader2,
  Minus,
  PanelRightClose,
  Plus,
  RotateCcw,
  Save,
  Scissors,
  SplitSquareHorizontal,
  Sparkles,
  SquarePen,
  TextCursorInput,
  TriangleAlert,
  X,
} from 'lucide-react';
import { useTranslation } from '../../i18n';
import * as api from '../../lib/api';
import { useResizablePanel } from '../../lib/useResizablePanel';
import { markdownComponents, rehypePlugins } from '../../components/chat/markdownComponents';
import { FilePreviewContext } from './filePreviewContext';

type PreviewMode = 'preview' | 'text' | 'edit' | 'split';

const INSTANT_TRANSITION = { duration: 0 };
const FILE_PREVIEW_WIDTH_KEY = 'file-preview-panel-width';
const FILE_PREVIEW_MIN_WIDTH = 560;
const FILE_PREVIEW_MAX_WIDTH = 1180;
const REMARK_PLUGINS = [remarkGfm];
const MAX_AGENT_SELECTION_CHARS = 24_000;

type TextSelectionState = {
  start: number;
  end: number;
  origin: 'editor' | 'preview';
};

type TextSelectionSummary = TextSelectionState & {
  text: string;
  startLine: number;
  endLine: number;
  charCount: number;
  lineCount: number;
};

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

function lineNumberAt(content: string, index: number): number {
  const safeIndex = Math.max(0, Math.min(index, content.length));
  let line = 1;
  for (let i = 0; i < safeIndex; i += 1) {
    if (content.charCodeAt(i) === 10) line += 1;
  }
  return line;
}

function getSelectionSummary(
  content: string,
  selection: TextSelectionState | null,
): TextSelectionSummary | null {
  if (!selection) return null;
  const start = Math.max(0, Math.min(selection.start, content.length));
  const end = Math.max(0, Math.min(selection.end, content.length));
  if (end <= start) return null;

  const text = content.slice(start, end);
  if (!text.trim()) return null;

  const startLine = lineNumberAt(content, start);
  const endLine = lineNumberAt(content, Math.max(start, end - 1));

  return {
    ...selection,
    start,
    end,
    text,
    startLine,
    endLine,
    charCount: text.length,
    lineCount: endLine - startLine + 1,
  };
}

function codeFenceFor(text: string): string {
  let fence = '```';
  while (text.includes(fence)) {
    fence += '`';
  }
  return fence;
}

function normalizeRenderedSelection(text: string): string {
  return text.replace(/\r\n?/g, '\n');
}

function isOfficeDocumentPreview(preview: api.FilePreview): boolean {
  return ['.docx', '.pptx', '.xlsx'].includes(preview.extension.toLowerCase());
}

function hasStructuredPreview(preview: api.FilePreview | null): boolean {
  return Boolean(preview?.structuredPreview);
}

function defaultModeForPreview(preview: api.FilePreview): PreviewMode {
  if (preview.structuredPreview) return 'preview';
  if (preview.editable) return 'edit';
  return 'preview';
}

function buildAgentEditPrompt({
  locale,
  preview,
  selection,
  instruction,
}: {
  locale: string;
  preview: api.FilePreview;
  selection: TextSelectionSummary;
  instruction: string;
}): string {
  const zh = locale.startsWith('zh');
  const fallbackInstruction = zh
    ? '请在不改变原意的前提下，优化这段文字的表达。'
    : 'Improve this passage without changing its intent.';
  const finalInstruction = instruction.trim() || fallbackInstruction;
  const lineRange =
    selection.startLine === selection.endLine
      ? `${selection.startLine}`
      : `${selection.startLine}-${selection.endLine}`;
  const fence = codeFenceFor(selection.text);
  const officeDocument = isOfficeDocumentPreview(preview);

  if (zh) {
    if (officeDocument) {
      return [
        '请使用 Python 文档 skill 直接修改下面这个 Office 文档中的选中文本片段。',
        '',
        `文件: ${preview.path}`,
        `文件名: ${preview.displayName}`,
        `来源: ${preview.sourceName}`,
        `提取文本行号: ${lineRange}`,
        `预览字符范围: ${selection.start}-${selection.end}`,
        `当前文件哈希: ${preview.hash}`,
        '',
        '用户修改要求:',
        finalInstruction,
        '',
        '选中文本:',
        fence,
        selection.text,
        fence,
        '',
        '执行规则:',
        '1. 先用 read_file 验证文档的提取文本仍然包含这段选中文本。',
        '2. 根据用户修改要求生成替换后的 new_text。',
        '3. 使用 doc-script-editor skill，通过 run_shell 调用 `python <SKILL_DIR>/scripts/edit_doc.py` 修改文档；run_shell 的 cwd 必须设为包含该文件的已注册来源目录，必要时先用 list_sources 确认。',
        '4. 如果运行环境不完整，先使用 prepare_document_tools 检查或准备必需依赖。',
        '5. 先执行 replace --dry-run 预览替换，再执行实际 replace；--find 必须是上方选中文本的精确内容。',
        '6. 不要使用 edit_file 修改 Office 二进制文档。',
        '7. 只替换这段文本；如果无法唯一定位、文本已变化或 Python skill 无法完成，请先停下来问我确认。',
        '8. 修改后运行 validate，并简要说明改了什么和如何回滚。',
      ].join('\n');
    }

    if (!preview.editable) {
      return [
        '请处理下面这个只读提取文本中的选中片段，并在工具支持时直接修改源文件。',
        '',
        `文件: ${preview.path}`,
        `文件名: ${preview.displayName}`,
        `来源: ${preview.sourceName}`,
        `提取文本行号: ${lineRange}`,
        `预览字符范围: ${selection.start}-${selection.end}`,
        `当前文件哈希: ${preview.hash}`,
        '',
        '用户修改要求:',
        finalInstruction,
        '',
        '选中文本:',
        fence,
        selection.text,
        fence,
        '',
        '执行规则:',
        '1. 先用 read_file 验证源文件内容或提取文本仍然匹配。',
        '2. 如果当前工具支持安全修改该文件格式，请直接修改源文件。',
        '3. 如果该格式不能被当前工具安全写回，请不要用 edit_file 强行修改；请给出替换后的文本并说明需要我确认下一步。',
        '4. 只处理这段选中文本，除非我的修改要求明确需要扩大范围。',
      ].join('\n');
    }

    return [
      '请直接修改下面这个已索引来源文件中的选中文本片段。',
      '',
      `文件: ${preview.path}`,
      `文件名: ${preview.displayName}`,
      `来源: ${preview.sourceName}`,
      `行号: ${lineRange}`,
      `预览字符范围: ${selection.start}-${selection.end}`,
      `当前文件哈希: ${preview.hash}`,
      '',
      '用户修改要求:',
      finalInstruction,
      '',
      '选中文本:',
      fence,
      selection.text,
      fence,
      '',
      '执行规则:',
      '1. 先用 read_file 验证文件内容和选中文本仍然匹配。',
      '2. 只替换这段选中文本，除非我的修改要求明确需要扩大范围。',
      '3. 使用 edit_file 修改文件，不要只给建议或改写稿。',
      '4. 如果文本已经变化、无法唯一定位，或文件不在已注册来源内，请先停下来问我确认。',
      '5. 修改后简要说明改了什么，并保留可回滚 checkpoint。',
    ].join('\n');
  }

  if (officeDocument) {
    return [
      'Please use the Python document skill to directly edit the selected text in this Office document.',
      '',
      `File: ${preview.path}`,
      `Display name: ${preview.displayName}`,
      `Source: ${preview.sourceName}`,
      `Extracted text line range: ${lineRange}`,
      `Preview character range: ${selection.start}-${selection.end}`,
      `Current file hash: ${preview.hash}`,
      '',
      'Requested change:',
      finalInstruction,
      '',
      'Selected text:',
      fence,
      selection.text,
      fence,
      '',
      'Execution rules:',
      '1. Use read_file first to verify that the extracted document text still contains this selected text.',
      '2. Generate the replacement new_text from the requested change.',
      '3. Use the doc-script-editor skill through run_shell by invoking `python <SKILL_DIR>/scripts/edit_doc.py`; set run_shell cwd to the registered source directory that contains this file, using list_sources first if needed.',
      '4. If the runtime is incomplete, use prepare_document_tools to check or prepare the required dependencies first.',
      '5. Run replace --dry-run first, then run the real replace. The --find value must be exactly the selected text above.',
      '6. Do not use edit_file on the Office binary document.',
      '7. Replace only this text. If it cannot be uniquely located, has changed, or the Python skill cannot complete the edit, stop and ask me to confirm.',
      '8. After editing, run validate and briefly summarize what changed and how to roll it back.',
    ].join('\n');
  }

  if (!preview.editable) {
    return [
      'Please work on the selected text from this read-only extracted preview and directly modify the source file only when the available tools safely support that format.',
      '',
      `File: ${preview.path}`,
      `Display name: ${preview.displayName}`,
      `Source: ${preview.sourceName}`,
      `Extracted text line range: ${lineRange}`,
      `Preview character range: ${selection.start}-${selection.end}`,
      `Current file hash: ${preview.hash}`,
      '',
      'Requested change:',
      finalInstruction,
      '',
      'Selected text:',
      fence,
      selection.text,
      fence,
      '',
      'Execution rules:',
      '1. Use read_file first to verify that the source file or extracted text still matches.',
      '2. If the current tools safely support editing this format, directly edit the source file.',
      '3. If the format cannot be safely written by the current tools, do not force an edit with edit_file. Provide the replacement text and ask me to confirm the next step.',
      '4. Work only on this selected text unless the requested change explicitly requires a wider edit.',
    ].join('\n');
  }

  return [
    'Please directly edit the selected text in this indexed source file.',
    '',
    `File: ${preview.path}`,
    `Display name: ${preview.displayName}`,
    `Source: ${preview.sourceName}`,
    `Line range: ${lineRange}`,
    `Preview character range: ${selection.start}-${selection.end}`,
    `Current file hash: ${preview.hash}`,
    '',
    'Requested change:',
    finalInstruction,
    '',
    'Selected text:',
    fence,
    selection.text,
    fence,
    '',
    'Execution rules:',
    '1. Use read_file first to verify that the file and selected text still match.',
    '2. Replace only this selected text unless the requested change explicitly requires a wider edit.',
    '3. Use edit_file to modify the file. Do not only provide advice or a rewritten draft.',
    '4. If the text has changed, cannot be uniquely located, or is outside a registered source, stop and ask me to confirm.',
    '5. After editing, briefly summarize what changed and keep the rollback checkpoint available.',
  ].join('\n');
}

function copyForLocale(locale: string) {
  const zh = locale.startsWith('zh');
  return {
    title: zh ? '文件预览' : 'File Preview',
    preview: zh ? '预览' : 'Preview',
    structured: zh ? '结构化' : 'Structured',
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
    resizePanel: zh ? '调整预览面板宽度' : 'Resize preview panel',
    loading: zh ? '正在读取文件...' : 'Reading file...',
    empty: zh ? '没有可预览的文本内容。' : 'No text content is available for preview.',
    unsupported: zh ? '这个文件暂时不能在应用内预览或编辑。' : 'This file cannot be previewed or edited inline yet.',
    sheets: zh ? '工作表' : 'sheets',
    rows: zh ? '行' : 'rows',
    columns: zh ? '列' : 'columns',
    formula: zh ? '公式' : 'Formula',
    truncatedPreview: zh ? '预览已截断' : 'Preview truncated',
    conflict: zh ? '文件已在磁盘上变化，请重新加载后再保存。' : 'The file changed on disk. Reload before saving.',
    saveFailed: zh ? '保存失败' : 'Save failed',
    loadFailed: zh ? '预览失败' : 'Preview failed',
    reindexFailed: zh ? '文件已保存，但重新索引失败' : 'Saved, but reindexing failed',
    dirty: zh ? '未保存' : 'Unsaved',
    lines: zh ? '行' : 'lines',
    pages: zh ? '页' : 'pages',
    page: zh ? '页' : 'Page',
    pageBreak: zh ? '分页' : 'Page break',
    zoomIn: zh ? '放大' : 'Zoom in',
    zoomOut: zh ? '缩小' : 'Zoom out',
    resetZoom: zh ? '重置缩放' : 'Reset zoom',
    source: zh ? '来源' : 'Source',
    encoding: zh ? '编码' : 'Encoding',
    discardPrompt: zh ? '当前文件有未保存修改，确定要关闭吗？' : 'This file has unsaved changes. Close anyway?',
    agentEdit: zh ? 'Agent 修改' : 'Agent Edit',
    selected: zh ? '已选择' : 'Selected',
    chars: zh ? '字符' : 'chars',
    lineRange: zh ? '行' : 'lines',
    agentInstructionPlaceholder: zh ? '告诉 Agent 要如何修改这段文字...' : 'Tell the agent how to change this selection...',
    askAgent: zh ? '让 Agent 修改' : 'Ask Agent to Edit',
    copyRequest: zh ? '复制请求' : 'Copy Request',
    requestCopied: zh ? '请求已复制' : 'Request copied',
    agentRequestSent: zh ? '已发送给 Agent' : 'Sent to agent',
    saveBeforeAgent: zh ? '请先保存当前草稿，再让 Agent 按磁盘文件修改。' : 'Save the current draft before asking the agent to edit the disk file.',
    selectionTooLarge: zh ? '选区较长，Agent 会先重新读取文件再定位。' : 'Large selection. The agent will re-read the file before locating it.',
    selectionMapFailed: zh ? '预览选区无法精确映射，请切到编辑或分屏后选择。' : 'That preview selection could not be mapped exactly. Select it in Edit or Split mode.',
    quickRewrite: zh ? '改写更清晰' : 'Rewrite Clearly',
    quickShorten: zh ? '压缩' : 'Shorten',
    quickFix: zh ? '修正语法' : 'Fix Grammar',
    quickTranslateZh: zh ? '翻译中文' : 'Translate to Chinese',
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

type PreviewLabels = ReturnType<typeof copyForLocale>;

function isSafeCssColor(value: string | null | undefined): string | undefined {
  if (!value) return undefined;
  if (/^#[0-9a-fA-F]{6}$/.test(value)) return value;
  return undefined;
}

function highlightColor(value: string | null | undefined): string | undefined {
  if (!value) return undefined;
  const normalized = value.toLowerCase();
  const named: Record<string, string> = {
    yellow: 'rgba(250, 204, 21, 0.22)',
    green: 'rgba(34, 197, 94, 0.18)',
    cyan: 'rgba(34, 211, 238, 0.18)',
    magenta: 'rgba(217, 70, 239, 0.2)',
    blue: 'rgba(59, 130, 246, 0.18)',
    red: 'rgba(239, 68, 68, 0.18)',
    darkyellow: 'rgba(202, 138, 4, 0.24)',
    darkgreen: 'rgba(22, 101, 52, 0.28)',
    darkcyan: 'rgba(14, 116, 144, 0.28)',
    darkmagenta: 'rgba(134, 25, 143, 0.28)',
    darkblue: 'rgba(30, 64, 175, 0.28)',
    darkred: 'rgba(153, 27, 27, 0.28)',
    lightgray: 'rgba(148, 163, 184, 0.18)',
    darkgray: 'rgba(71, 85, 105, 0.28)',
  };
  return named[normalized] ?? isSafeCssColor(value);
}

function textAlignClass(alignment: string | null | undefined): string {
  switch (alignment) {
    case 'center':
      return 'text-center';
    case 'right':
      return 'text-right';
    case 'both':
      return 'text-justify';
    default:
      return 'text-left';
  }
}

function runSizeClass(value: string | null | undefined): string {
  switch (value) {
    case 'small':
      return 'text-xs';
    case 'large':
      return 'text-base';
    case 'xlarge':
      return 'text-lg';
    default:
      return '';
  }
}

function DocumentRuns({ runs }: { runs: api.DocumentPreviewRun[] }) {
  return (
    <>
      {runs.map((run, index) => {
        const style: CSSProperties = {
          color: isSafeCssColor(run.color),
          backgroundColor: highlightColor(run.backgroundColor),
        };
        const className = [
          run.bold ? 'font-semibold' : '',
          run.italic ? 'italic' : '',
          run.underline || run.hyperlink ? 'underline underline-offset-2' : '',
          run.hyperlink ? 'text-accent' : '',
          runSizeClass(run.fontSize),
        ]
          .filter(Boolean)
          .join(' ');
        const text = run.text || ' ';

        if (run.hyperlink) {
          return (
            <a
              key={`${index}-${run.text}`}
              href={run.hyperlink}
              target="_blank"
              rel="noreferrer"
              className={className}
              style={style}
            >
              {text}
            </a>
          );
        }

        return (
          <span key={`${index}-${run.text}`} className={className} style={style}>
            {text}
          </span>
        );
      })}
    </>
  );
}

function DocumentBlockView({
  block,
  assetMap,
  labels,
}: {
  block: api.DocumentPreviewBlock;
  assetMap: Map<string, api.PreviewAsset>;
  labels: PreviewLabels;
}) {
  switch (block.type) {
    case 'heading': {
      const Tag = (`h${Math.min(Math.max(block.level, 1), 6)}`) as
        | 'h1'
        | 'h2'
        | 'h3'
        | 'h4'
        | 'h5'
        | 'h6';
      const headingClass =
        block.level <= 1
          ? 'mt-7 text-2xl font-semibold leading-tight'
          : block.level === 2
            ? 'mt-6 text-xl font-semibold leading-tight'
            : 'mt-5 text-base font-semibold leading-snug';
      return (
        <Tag className={`${headingClass} first:mt-0 ${textAlignClass(block.alignment)}`}>
          <DocumentRuns runs={block.runs} />
        </Tag>
      );
    }
    case 'paragraph':
      return (
        <p className={`my-3 whitespace-pre-wrap text-sm leading-7 ${textAlignClass(block.alignment)}`}>
          <DocumentRuns runs={block.runs} />
        </p>
      );
    case 'list': {
      const ListTag = block.ordered ? 'ol' : 'ul';
      return (
        <ListTag
          className={`my-3 space-y-1 text-sm leading-7 ${
            block.ordered ? 'list-decimal' : 'list-disc'
          }`}
          style={{ paddingLeft: `${1.5 + block.level * 1.25}rem` }}
        >
          {block.items.map((item, index) => (
            <li key={index}>
              <DocumentRuns runs={item.runs} />
            </li>
          ))}
        </ListTag>
      );
    }
    case 'table':
      return (
        <div className="my-4 overflow-x-auto rounded-md border border-border">
          <table className="min-w-full border-collapse text-sm">
            <tbody>
              {block.rows.map((row, rowIndex) => (
                <tr key={rowIndex} className="border-b border-border last:border-b-0">
                  {row.cells.map((cell, cellIndex) => (
                    <td
                      key={cellIndex}
                      className="min-w-32 border-r border-border bg-surface-0 px-3 py-2 align-top last:border-r-0"
                    >
                      {cell.blocks.length > 0 ? (
                        cell.blocks.map((child, childIndex) => (
                          <DocumentBlockView
                            key={childIndex}
                            block={child}
                            assetMap={assetMap}
                            labels={labels}
                          />
                        ))
                      ) : (
                        <span className="text-text-tertiary"> </span>
                      )}
                    </td>
                  ))}
                </tr>
              ))}
            </tbody>
          </table>
        </div>
      );
    case 'image': {
      const asset = assetMap.get(block.assetId);
      if (!asset) {
        return (
          <div className="my-4 rounded-md border border-border bg-surface-0 px-3 py-2 text-xs text-text-tertiary">
            {block.alt ?? block.assetId}
          </div>
        );
      }
      return (
        <figure className="my-5">
          <img
            src={convertFileSrc(asset.path)}
            alt={block.alt ?? asset.id}
            className="max-h-[520px] max-w-full rounded-md border border-border bg-surface-0 object-contain"
          />
          {(block.alt || asset.mimeType) && (
            <figcaption className="mt-1 text-[11px] text-text-tertiary">
              {block.alt ?? asset.mimeType}
            </figcaption>
          )}
        </figure>
      );
    }
    case 'pageBreak':
      return (
        <div className="my-7 flex items-center gap-3 text-[11px] uppercase tracking-wide text-text-tertiary">
          <span className="h-px flex-1 bg-border" />
          <span>{labels.pageBreak}</span>
          <span className="h-px flex-1 bg-border" />
        </div>
      );
    case 'unsupported':
      return (
        <div className="my-3 rounded-md border border-warning/30 bg-warning/10 px-3 py-2 text-xs text-warning">
          {block.message}
        </div>
      );
    default:
      return null;
  }
}

function StructuredDocumentPreview({
  preview,
  labels,
  onMouseUp,
}: {
  preview: api.DocumentStructuredPreview;
  labels: PreviewLabels;
  onMouseUp: () => void;
}) {
  const assetMap = useMemo(
    () => new Map(preview.assets.map((asset) => [asset.id, asset])),
    [preview.assets],
  );

  return (
    <div
      data-testid="file-preview-structured-document"
      className="h-full overflow-auto bg-surface-0 px-4 py-5"
      onMouseUp={onMouseUp}
    >
      <article className="mx-auto min-h-full max-w-[900px] rounded-md border border-border/70 bg-surface-1 px-6 py-6 text-text-primary shadow-[0_18px_45px_rgba(0,0,0,0.18)] sm:px-8 sm:py-7">
        {preview.blocks.map((block, index) => (
          <DocumentBlockView key={index} block={block} assetMap={assetMap} labels={labels} />
        ))}
      </article>
    </div>
  );
}

function columnName(column: number): string {
  let value = column + 1;
  let label = '';
  while (value > 0) {
    value -= 1;
    label = String.fromCharCode(65 + (value % 26)) + label;
    value = Math.floor(value / 26);
  }
  return label;
}

function WorkbookPreview({
  preview,
  labels,
  onMouseUp,
}: {
  preview: api.WorkbookStructuredPreview;
  labels: PreviewLabels;
  onMouseUp: () => void;
}) {
  const [selectedSheetIndex, setSelectedSheetIndex] = useState(0);

  useEffect(() => {
    setSelectedSheetIndex(0);
  }, [preview]);

  const sheet = preview.sheets[Math.min(selectedSheetIndex, Math.max(preview.sheets.length - 1, 0))];
  const cellMap = useMemo(() => {
    const map = new Map<string, api.WorkbookPreviewCell>();
    for (const cell of sheet?.cells ?? []) {
      map.set(`${cell.row}:${cell.column}`, cell);
    }
    return map;
  }, [sheet]);
  const mergeInfo = useMemo(() => {
    const starts = new Map<string, { rowSpan: number; colSpan: number }>();
    const covered = new Set<string>();
    for (const range of sheet?.mergedRanges ?? []) {
      const rowSpan = range.endRow - range.startRow + 1;
      const colSpan = range.endColumn - range.startColumn + 1;
      starts.set(`${range.startRow}:${range.startColumn}`, { rowSpan, colSpan });
      for (let row = range.startRow; row <= range.endRow; row += 1) {
        for (let col = range.startColumn; col <= range.endColumn; col += 1) {
          if (row !== range.startRow || col !== range.startColumn) {
            covered.add(`${row}:${col}`);
          }
        }
      }
    }
    return { starts, covered };
  }, [sheet]);

  if (!sheet) {
    return (
      <div className="flex h-full items-center justify-center px-6 text-center text-sm text-text-tertiary">
        {labels.empty}
      </div>
    );
  }

  const rowCount = Math.max(sheet.previewRowCount, 1);
  const columnCount = Math.max(sheet.previewColumnCount, 1);
  const columns = Array.from({ length: columnCount }, (_, index) => index);
  const rows = Array.from({ length: rowCount }, (_, index) => index);
  const gridStyle: CSSProperties = {
    gridTemplateColumns: `3rem repeat(${columnCount}, minmax(7rem, 1fr))`,
    gridTemplateRows: `2rem repeat(${rowCount}, minmax(2rem, auto))`,
  };

  return (
    <div data-testid="file-preview-workbook" className="flex h-full min-h-0 flex-col bg-surface-0">
      <div className="shrink-0 border-b border-border bg-surface-1/95 px-4 py-2">
        <div className="flex min-w-0 items-center gap-2">
          <FileSpreadsheet size={14} className="shrink-0 text-accent" />
          <span className="shrink-0 text-xs font-medium text-text-primary">
            {preview.sheets.length} {labels.sheets}
          </span>
          {preview.truncated && (
            <span className="rounded-full border border-warning/25 bg-warning/10 px-2 py-0.5 text-[10px] font-medium text-warning">
              {labels.truncatedPreview}
            </span>
          )}
          <div className="min-w-0 flex-1 overflow-x-auto">
            <div className="flex gap-1">
              {preview.sheets.map((candidate, index) => (
                <button
                  key={`${candidate.index}-${candidate.name}`}
                  type="button"
                  onClick={() => setSelectedSheetIndex(index)}
                  className={`h-7 shrink-0 rounded-md px-2.5 text-[11px] font-medium transition-colors ${
                    index === selectedSheetIndex
                      ? 'bg-accent text-white'
                      : 'text-text-secondary hover:bg-surface-3 hover:text-text-primary'
                  }`}
                >
                  {candidate.name}
                </button>
              ))}
            </div>
          </div>
          <span className="hidden shrink-0 text-[11px] text-text-tertiary sm:inline">
            {sheet.rowCount} {labels.rows} · {sheet.columnCount} {labels.columns}
          </span>
        </div>
      </div>

      <div className="min-h-0 flex-1 overflow-auto p-4" onMouseUp={onMouseUp}>
        <div className="grid min-w-max rounded-md border border-border bg-surface-1 text-xs" style={gridStyle}>
          <div className="sticky left-0 top-0 z-30 border-b border-r border-border bg-surface-2" />
          {columns.map((column) => (
            <div
              key={`col-${column}`}
              className="sticky top-0 z-20 flex h-8 items-center justify-center border-b border-r border-border bg-surface-2 font-medium text-text-tertiary"
              style={{ gridColumn: column + 2, gridRow: 1 }}
            >
              {columnName(column)}
            </div>
          ))}
          {rows.map((row) => (
            <div
              key={`row-${row}`}
              className="sticky left-0 z-10 flex h-8 items-center justify-end border-b border-r border-border bg-surface-2 px-2 font-medium text-text-tertiary"
              style={{ gridColumn: 1, gridRow: row + 2 }}
            >
              {row + 1}
            </div>
          ))}
          {rows.flatMap((row) =>
            columns.map((column) => {
              if (mergeInfo.covered.has(`${row}:${column}`)) return null;
              const cell = cellMap.get(`${row}:${column}`);
              const merged = mergeInfo.starts.get(`${row}:${column}`);
              const style: CSSProperties = {
                gridColumn: `${column + 2} / span ${merged?.colSpan ?? 1}`,
                gridRow: `${row + 2} / span ${merged?.rowSpan ?? 1}`,
              };
              return (
                <div
                  key={`${row}-${column}`}
                  className={`min-h-8 overflow-hidden border-b border-r border-border px-2 py-1.5 leading-5 ${
                    cell?.formula ? 'bg-accent/5' : 'bg-surface-1'
                  }`}
                  style={style}
                  title={cell?.formula ? `${labels.formula}: ${cell.formula}` : cell?.value}
                >
                  {cell ? (
                    <div className="flex min-w-0 items-start gap-1.5">
                      {cell.formula && (
                        <span className="mt-0.5 shrink-0 rounded border border-accent/30 px-1 text-[9px] font-semibold uppercase text-accent">
                          fx
                        </span>
                      )}
                      <span className="min-w-0 whitespace-pre-wrap break-words text-text-primary">
                        {cell.value}
                      </span>
                    </div>
                  ) : (
                    <span className="text-text-tertiary"> </span>
                  )}
                </div>
              );
            }),
          )}
        </div>
      </div>
    </div>
  );
}

function StructuredPreviewRenderer({
  preview,
  labels,
  onMouseUp,
}: {
  preview: api.StructuredPreview;
  labels: PreviewLabels;
  onMouseUp: () => void;
}) {
  if (preview.type === 'workbook') {
    return <WorkbookPreview preview={preview} labels={labels} onMouseUp={onMouseUp} />;
  }
  return <StructuredDocumentPreview preview={preview} labels={labels} onMouseUp={onMouseUp} />;
}

function OfficeRenderedPreview({
  rendered,
  labels,
}: {
  rendered: api.RenderedPreview;
  labels: PreviewLabels;
}) {
  const [zoom, setZoom] = useState(1);

  useEffect(() => {
    setZoom(1);
  }, [rendered]);

  const zoomPercent = Math.round(zoom * 100);
  const pageWidth = Math.round(900 * zoom);
  const pageSummary = rendered.truncated
    ? `${rendered.pageCount}+ ${labels.pages}`
    : `${rendered.pageCount} ${labels.pages}`;

  return (
    <div data-testid="file-preview-rendered-content" className="flex h-full min-h-0 flex-col bg-surface-0">
      <div className="shrink-0 border-b border-border bg-surface-1/95 px-4 py-2 backdrop-blur">
        <div className="flex items-center gap-2">
          <div className="flex min-w-0 items-center gap-2 text-xs text-text-tertiary">
            <ImageIcon size={14} className="text-accent" />
            <span className="whitespace-nowrap">{pageSummary}</span>
            <span className="hidden whitespace-nowrap sm:inline">{rendered.dpi} DPI</span>
          </div>
          <div className="flex-1" />
          <div className="flex items-center rounded-md border border-border bg-surface-2 p-0.5">
            <button
              type="button"
              onClick={() => setZoom((value) => Math.max(0.5, Number((value - 0.1).toFixed(2))))}
              className="inline-flex h-7 w-7 items-center justify-center rounded text-text-secondary transition-colors hover:bg-surface-3 hover:text-text-primary"
              title={labels.zoomOut}
              aria-label={labels.zoomOut}
            >
              <Minus size={14} />
            </button>
            <button
              type="button"
              onClick={() => setZoom(1)}
              className="h-7 min-w-12 rounded px-2 text-[11px] font-medium text-text-secondary transition-colors hover:bg-surface-3 hover:text-text-primary"
              title={labels.resetZoom}
              aria-label={labels.resetZoom}
            >
              {zoomPercent}%
            </button>
            <button
              type="button"
              onClick={() => setZoom((value) => Math.min(1.8, Number((value + 0.1).toFixed(2))))}
              className="inline-flex h-7 w-7 items-center justify-center rounded text-text-secondary transition-colors hover:bg-surface-3 hover:text-text-primary"
              title={labels.zoomIn}
              aria-label={labels.zoomIn}
            >
              <Plus size={14} />
            </button>
          </div>
        </div>
      </div>

      <div className="min-h-0 flex-1 overflow-auto px-4 py-5">
        <div className="mx-auto flex max-w-full flex-col items-center gap-5">
          {rendered.pages.map((page) => (
            <figure
              key={`${page.page}-${page.path}`}
              data-testid="file-preview-rendered-page"
              className="m-0"
              style={{
                width: pageWidth,
                maxWidth: zoom <= 1 ? '100%' : undefined,
              }}
            >
              <figcaption className="mb-1 flex items-center justify-between px-1 text-[11px] text-text-tertiary">
                <span>
                  {labels.page} {page.page}
                </span>
              </figcaption>
              <div className="overflow-hidden rounded-md border border-border/70 bg-white shadow-[0_18px_45px_rgba(0,0,0,0.28)]">
                <img
                  src={convertFileSrc(page.path)}
                  alt={`${labels.page} ${page.page}`}
                  draggable={false}
                  className="block w-full select-none"
                />
              </div>
            </figure>
          ))}
        </div>
      </div>
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
  const navigate = useNavigate();
  const labels = useMemo(() => copyForLocale(locale), [locale]);
  const shouldReduceMotion = useReducedMotion();
  const [open, setOpen] = useState(false);
  const [activePath, setActivePath] = useState<string | null>(null);
  const [preview, setPreview] = useState<api.FilePreview | null>(null);
  const [draft, setDraft] = useState('');
  const [textSelection, setTextSelection] = useState<TextSelectionState | null>(null);
  const [agentInstruction, setAgentInstruction] = useState('');
  const [copiedAgentRequest, setCopiedAgentRequest] = useState(false);
  const [mode, setMode] = useState<PreviewMode>('preview');
  const [loading, setLoading] = useState(false);
  const [saving, setSaving] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [copiedPath, setCopiedPath] = useState(false);
  const {
    size: previewPanelWidth,
    setSize: setPreviewPanelWidth,
    startResize: startPreviewPanelResize,
    isResizing: isPreviewPanelResizing,
  } = useResizablePanel({
    storageKey: FILE_PREVIEW_WIDTH_KEY,
    defaultSize: 860,
    minSize: FILE_PREVIEW_MIN_WIDTH,
    maxSize: FILE_PREVIEW_MAX_WIDTH,
    direction: -1,
  });
  const dirty = Boolean(preview?.editable && draft !== (preview.content ?? ''));
  const dirtyRef = useRef(false);

  useEffect(() => {
    dirtyRef.current = dirty;
  }, [dirty]);

  const loadFile = useCallback(
    async (
      path: string,
      options: { preferredMode?: PreviewMode } = {},
    ) => {
      setLoading(true);
      setError(null);
      setActivePath(path);
      try {
        const next = await api.previewFile(path);
        setPreview(next);
        setDraft(next.content ?? '');
        setTextSelection(null);
        setAgentInstruction('');
        setCopiedAgentRequest(false);
        setMode(options.preferredMode ?? defaultModeForPreview(next));
        setActivePath(next.path);
      } catch (err) {
        const message = err instanceof Error ? err.message : String(err);
        setPreview(null);
        setDraft('');
        setTextSelection(null);
        setAgentInstruction('');
        setError(message);
        toast.error(`${labels.loadFailed}: ${message}`);
      } finally {
        setLoading(false);
      }
    },
    [labels.loadFailed],
  );

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

  const handlePreviewPanelResizeKey = useCallback((event: ReactKeyboardEvent<HTMLDivElement>) => {
    if (event.key === 'ArrowLeft') {
      event.preventDefault();
      setPreviewPanelWidth(previewPanelWidth + 16);
    } else if (event.key === 'ArrowRight') {
      event.preventDefault();
      setPreviewPanelWidth(previewPanelWidth - 16);
    } else if (event.key === 'Home') {
      event.preventDefault();
      setPreviewPanelWidth(FILE_PREVIEW_MIN_WIDTH);
    } else if (event.key === 'End') {
      event.preventDefault();
      setPreviewPanelWidth(FILE_PREVIEW_MAX_WIDTH);
    }
  }, [previewPanelWidth, setPreviewPanelWidth]);

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

  const selectedText = useMemo(
    () => getSelectionSummary(draft, textSelection),
    [draft, textSelection],
  );

  const quickActions = useMemo(
    () => [
      {
        id: 'rewrite',
        label: labels.quickRewrite,
        instruction: locale.startsWith('zh')
          ? '请把选中文本改写得更清晰、更自然，保留原意。'
          : 'Rewrite the selected text to be clearer and more natural while preserving the intent.',
        icon: <Sparkles size={13} />,
      },
      {
        id: 'shorten',
        label: labels.quickShorten,
        instruction: locale.startsWith('zh')
          ? '请压缩选中文本，保留关键信息，减少冗余。'
          : 'Shorten the selected text, preserving the key information and removing redundancy.',
        icon: <Scissors size={13} />,
      },
      {
        id: 'fix',
        label: labels.quickFix,
        instruction: locale.startsWith('zh')
          ? '请修正选中文本中的语法、错别字和不通顺表达。'
          : 'Fix grammar, typos, and awkward phrasing in the selected text.',
        icon: <TextCursorInput size={13} />,
      },
      {
        id: 'translate-zh',
        label: labels.quickTranslateZh,
        instruction: locale.startsWith('zh')
          ? '请将选中文本翻译成自然、准确的中文。'
          : 'Translate the selected text into natural, accurate Chinese.',
        icon: <Languages size={13} />,
      },
    ],
    [labels.quickFix, labels.quickRewrite, labels.quickShorten, labels.quickTranslateZh, locale],
  );

  const updateSelectionFromEditor = useCallback((target: HTMLTextAreaElement) => {
    const start = Math.min(target.selectionStart, target.selectionEnd);
    const end = Math.max(target.selectionStart, target.selectionEnd);
    if (end <= start) {
      setTextSelection(null);
      setCopiedAgentRequest(false);
      return;
    }
    setTextSelection({ start, end, origin: 'editor' });
    setCopiedAgentRequest(false);
  }, []);

  const captureRenderedSelection = useCallback(() => {
    if (!preview || !draft) return;
    const raw = window.getSelection()?.toString() ?? '';
    const selected = normalizeRenderedSelection(raw);
    if (!selected.trim()) return;

    const start = draft.indexOf(selected);
    if (start < 0) {
      setTextSelection(null);
      setCopiedAgentRequest(false);
      toast.info(labels.selectionMapFailed);
      return;
    }

    setTextSelection({ start, end: start + selected.length, origin: 'preview' });
    setCopiedAgentRequest(false);
  }, [draft, labels.selectionMapFailed, preview]);

  const updateDraft = useCallback((value: string) => {
    setDraft(value);
    setTextSelection(null);
    setCopiedAgentRequest(false);
  }, []);

  const buildCurrentAgentPrompt = useCallback(() => {
    if (!preview || !selectedText) return '';
    return buildAgentEditPrompt({
      locale,
      preview,
      selection: selectedText,
      instruction: agentInstruction,
    });
  }, [agentInstruction, locale, preview, selectedText]);

  const copyAgentRequest = useCallback(async () => {
    const prompt = buildCurrentAgentPrompt();
    if (!prompt) return;
    await navigator.clipboard.writeText(prompt);
    setCopiedAgentRequest(true);
    setTimeout(() => setCopiedAgentRequest(false), 1600);
    toast.success(labels.requestCopied);
  }, [buildCurrentAgentPrompt, labels.requestCopied]);

  const sendSelectionToAgent = useCallback(() => {
    if (!preview || !selectedText || dirty) return;
    const prompt = buildCurrentAgentPrompt();
    if (!prompt) return;
    navigate('/chat', {
      state: {
        initialMessage: prompt,
        sourceIds: [preview.sourceId],
      },
    });
    setOpen(false);
    toast.success(labels.agentRequestSent);
  }, [buildCurrentAgentPrompt, dirty, labels.agentRequestSent, navigate, preview, selectedText]);

  useEffect(() => {
    if (!textSelection) return;
    if (textSelection.start >= draft.length || textSelection.end > draft.length) {
      setTextSelection(null);
    }
  }, [draft.length, textSelection]);

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
  const hasStructured = hasStructuredPreview(preview);
  const hasRenderedPreview = Boolean(preview?.renderedPreview?.pages?.length);
  const canShowPreview = Boolean(hasStructured || hasRenderedPreview || preview?.content);
  const metadataBits = preview
    ? [
        formatBytes(preview.sizeBytes),
        preview.renderedPreview?.pageCount
          ? `${preview.renderedPreview.pageCount}${preview.renderedPreview.truncated ? '+' : ''} ${labels.pages}`
          : '',
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
          <>
            <motion.div
              key="file-preview-backdrop"
              initial={shouldReduceMotion ? false : { opacity: 0 }}
              animate={{ opacity: 1 }}
              exit={{ opacity: 0 }}
              transition={shouldReduceMotion ? INSTANT_TRANSITION : { duration: 0.15 }}
              className="fixed inset-0 z-50 bg-black/35 backdrop-blur-[1px]"
              onClick={close}
              aria-hidden="true"
            />
            <motion.aside
              key="file-preview-panel"
              initial={shouldReduceMotion ? false : { x: '100%', opacity: 0.8 }}
              animate={{ x: 0, opacity: 1 }}
              exit={shouldReduceMotion ? { opacity: 0 } : { x: '100%', opacity: 0.8 }}
              transition={shouldReduceMotion || isPreviewPanelResizing ? INSTANT_TRANSITION : { duration: 0.24, ease: [0.16, 1, 0.3, 1] }}
              className="fixed inset-y-0 right-0 z-[51] flex max-w-full flex-col border-l border-border bg-surface-1 shadow-2xl"
              style={{ width: previewPanelWidth }}
              role="dialog"
              aria-modal="true"
              aria-label={labels.title}
            >
            <div
              role="separator"
              aria-orientation="vertical"
              aria-valuemin={FILE_PREVIEW_MIN_WIDTH}
              aria-valuemax={FILE_PREVIEW_MAX_WIDTH}
              aria-valuenow={previewPanelWidth}
              tabIndex={0}
              onPointerDown={startPreviewPanelResize}
              onKeyDown={handlePreviewPanelResizeKey}
              className="absolute left-0 top-0 h-full w-2 -translate-x-1 cursor-col-resize touch-none
                bg-transparent outline-none transition-colors hover:bg-accent/25 focus-visible:bg-accent/35"
              title={labels.resizePanel}
            />
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
                    icon={
                      preview?.structuredPreview?.type === 'workbook'
                        ? <FileSpreadsheet size={14} />
                        : hasStructured
                          ? <ListTree size={14} />
                          : <Eye size={14} />
                    }
                    label={
                      hasStructured
                        ? labels.structured
                        : preview?.kind === 'document'
                          ? labels.extracted
                          : labels.preview
                    }
                    onClick={() => {
                      setMode('preview');
                    }}
                  />
                  {(hasStructured || hasRenderedPreview) && preview?.content && (
                    <ModeButton
                      active={mode === 'text'}
                      icon={<FileText size={14} />}
                      label={labels.extracted}
                      onClick={() => setMode('text')}
                    />
                  )}
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
                      onClick={() => {
                        setDraft(preview.content ?? '');
                        setTextSelection(null);
                        setAgentInstruction('');
                        setCopiedAgentRequest(false);
                      }}
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
                        if (preview) {
                          void loadFile(preview.path, { preferredMode: mode });
                        }
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
                  data-testid="file-preview-editor"
                  value={draft}
                  onChange={(event) => updateDraft(event.target.value)}
                  onSelect={(event) => updateSelectionFromEditor(event.currentTarget)}
                  onKeyUp={(event) => updateSelectionFromEditor(event.currentTarget)}
                  onMouseUp={(event) => updateSelectionFromEditor(event.currentTarget)}
                  spellCheck={false}
                  className="h-full w-full resize-none border-0 bg-surface-0 px-4 py-3 font-mono text-xs leading-5 text-text-primary outline-none placeholder:text-text-tertiary"
                />
              ) : mode === 'split' && preview.editable && preview.kind === 'markdown' ? (
                <div className="grid h-full grid-cols-1 md:grid-cols-2">
                  <textarea
                    data-testid="file-preview-editor"
                    value={draft}
                    onChange={(event) => updateDraft(event.target.value)}
                    onSelect={(event) => updateSelectionFromEditor(event.currentTarget)}
                    onKeyUp={(event) => updateSelectionFromEditor(event.currentTarget)}
                    onMouseUp={(event) => updateSelectionFromEditor(event.currentTarget)}
                    spellCheck={false}
                    className="h-full w-full resize-none border-0 border-r border-border bg-surface-0 px-4 py-3 font-mono text-xs leading-5 text-text-primary outline-none placeholder:text-text-tertiary md:border-r"
                  />
                  <div className="h-full overflow-auto bg-surface-1">
                    <MarkdownPreview content={draft} />
                  </div>
                </div>
              ) : canShowPreview ? (
                mode === 'preview' && preview.structuredPreview ? (
                  <StructuredPreviewRenderer
                    preview={preview.structuredPreview}
                    labels={labels}
                    onMouseUp={captureRenderedSelection}
                  />
                ) : hasRenderedPreview && mode === 'preview' && preview.renderedPreview ? (
                  <OfficeRenderedPreview rendered={preview.renderedPreview} labels={labels} />
                ) : preview.content ? (
                <div
                  data-testid="file-preview-readable-content"
                  className="h-full overflow-auto"
                  onMouseUp={captureRenderedSelection}
                >
                  {preview.kind === 'markdown' ? (
                    <MarkdownPreview content={preview.editable ? draft : content} />
                  ) : (
                    <TextPreview content={preview.editable ? draft : content} />
                  )}
                </div>
                ) : (
                  <div className="flex h-full items-center justify-center px-6 text-center text-sm text-text-tertiary">
                    {labels.empty}
                  </div>
                )
              ) : (
                <div className="flex h-full items-center justify-center px-6 text-center text-sm text-text-tertiary">
                  {preview.kind === 'binary' ? labels.unsupported : labels.empty}
                </div>
              )}
            </div>

            <AnimatePresence>
              {preview && selectedText && (
                <motion.div
                  key="agent-selection-panel"
                  initial={shouldReduceMotion ? false : { y: 16, opacity: 0 }}
                  animate={{ y: 0, opacity: 1 }}
                  exit={shouldReduceMotion ? { opacity: 0 } : { y: 16, opacity: 0 }}
                  transition={shouldReduceMotion ? INSTANT_TRANSITION : { duration: 0.18, ease: [0.16, 1, 0.3, 1] }}
                  data-testid="file-preview-agent-panel"
                  className="shrink-0 border-t border-border bg-surface-1/95 px-4 py-3 shadow-[0_-12px_28px_rgba(0,0,0,0.16)] backdrop-blur"
                >
                  <div className="flex items-start gap-3">
                    <div className="mt-0.5 flex h-8 w-8 shrink-0 items-center justify-center rounded-md border border-accent/25 bg-accent/10 text-accent">
                      <BotMessageSquare size={16} />
                    </div>
                    <div className="min-w-0 flex-1">
                      <div className="flex flex-wrap items-center gap-2">
                        <span className="text-xs font-semibold text-text-primary">{labels.agentEdit}</span>
                        <span className="rounded-full border border-border bg-surface-2 px-2 py-0.5 text-[10px] font-medium text-text-tertiary">
                          {labels.selected} {selectedText.charCount} {labels.chars} · {labels.lineRange}{' '}
                          {selectedText.startLine === selectedText.endLine
                            ? selectedText.startLine
                            : `${selectedText.startLine}-${selectedText.endLine}`}
                        </span>
                      </div>

                      {(dirty || selectedText.charCount > MAX_AGENT_SELECTION_CHARS) && (
                        <p className={`mt-1 text-[11px] ${dirty ? 'text-warning' : 'text-text-tertiary'}`}>
                          {dirty ? labels.saveBeforeAgent : labels.selectionTooLarge}
                        </p>
                      )}

                      <div className="mt-2 flex flex-wrap gap-1.5">
                        {quickActions.map((action) => (
                          <button
                            key={action.id}
                            type="button"
                            onClick={() => {
                              setAgentInstruction(action.instruction);
                              setCopiedAgentRequest(false);
                            }}
                            className="inline-flex h-7 items-center gap-1.5 rounded-md border border-border bg-surface-2 px-2 text-[11px] font-medium text-text-secondary transition-colors hover:border-accent/40 hover:bg-accent/10 hover:text-text-primary"
                          >
                            {action.icon}
                            <span>{action.label}</span>
                          </button>
                        ))}
                      </div>

                      <div className="mt-2 flex flex-col gap-2 sm:flex-row">
                        <input
                          data-testid="file-preview-agent-instruction"
                          value={agentInstruction}
                          onChange={(event) => {
                            setAgentInstruction(event.target.value);
                            setCopiedAgentRequest(false);
                          }}
                          onKeyDown={(event) => {
                            if (event.key === 'Enter' && !event.nativeEvent.isComposing) {
                              event.preventDefault();
                              sendSelectionToAgent();
                            }
                          }}
                          placeholder={labels.agentInstructionPlaceholder}
                          className="h-9 min-w-0 flex-1 rounded-md border border-border bg-surface-0 px-3 text-xs text-text-primary outline-none transition-colors placeholder:text-text-tertiary focus:border-accent/60"
                        />
                        <div className="flex shrink-0 gap-2">
                          {dirty && (
                            <button
                              type="button"
                              disabled={saving}
                              onClick={save}
                              className="inline-flex h-9 items-center gap-1.5 rounded-md border border-border bg-surface-2 px-3 text-xs font-medium text-text-secondary transition-colors hover:bg-surface-3 hover:text-text-primary disabled:pointer-events-none disabled:opacity-40"
                            >
                              {saving ? <Loader2 size={14} className="animate-spin" /> : <Save size={14} />}
                              {labels.save}
                            </button>
                          )}
                          <button
                            type="button"
                            disabled={dirty}
                            onClick={copyAgentRequest}
                            data-testid="file-preview-agent-copy"
                            className="inline-flex h-9 items-center justify-center rounded-md border border-border bg-surface-2 px-3 text-text-secondary transition-colors hover:bg-surface-3 hover:text-text-primary disabled:pointer-events-none disabled:opacity-40"
                            title={labels.copyRequest}
                            aria-label={labels.copyRequest}
                          >
                            {copiedAgentRequest ? <Check size={15} className="text-success" /> : <Copy size={15} />}
                          </button>
                          <button
                            type="button"
                            disabled={dirty}
                            onClick={sendSelectionToAgent}
                            data-testid="file-preview-agent-send"
                            className="inline-flex h-9 items-center gap-1.5 rounded-md bg-accent px-3 text-xs font-medium text-white transition-colors hover:bg-accent-hover disabled:pointer-events-none disabled:opacity-40"
                          >
                            <BotMessageSquare size={14} />
                            {labels.askAgent}
                          </button>
                        </div>
                      </div>
                    </div>
                  </div>
                </motion.div>
              )}
            </AnimatePresence>
            </motion.aside>
          </>
        )}
      </AnimatePresence>
    </FilePreviewContext.Provider>
  );
}
