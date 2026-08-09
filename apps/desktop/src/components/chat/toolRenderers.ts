import type { ToolRenderKind } from '../../types/conversation';

export interface ToolRendererDescriptor {
  kind: ToolRenderKind;
  matches: (toolName: string) => boolean;
  boardOnly?: boolean;
}

const normalizeToolName = (toolName?: string | null): string =>
  (toolName ?? '').trim().toLowerCase();

export const toolRenderers: ToolRendererDescriptor[] = [
  {
    kind: 'plan',
    boardOnly: true,
    matches: (name) => name === 'update_plan',
  },
  {
    kind: 'fileChange',
    matches: (name) =>
      name.includes('edit_file') ||
      name.includes('create_file') ||
      name.includes('multi_edit') ||
      name.includes('write_note') ||
      name.includes('apply_patch') ||
      name.includes('download_asset'),
  },
  {
    kind: 'subagent',
    matches: (name) => [
      'spawn_subagent',
      'spawn_subagent_batch',
      'observe_subagent',
      'wait_subagent',
      'send_subagent_input',
      'cancel_subagent',
      'close_subagent',
    ].includes(name),
  },
  {
    kind: 'image',
    matches: (name) => name === 'generate_image',
  },
  {
    kind: 'commandExecution',
    matches: (name) => name === 'run_shell',
  },
  {
    kind: 'search',
    matches: (name) =>
      name.includes('search') ||
      name === 'glob_files' ||
      name === 'list_dir' ||
      name === 'list_documents' ||
      name === 'list_sources',
  },
];

export function resolveToolRenderKind(
  toolName?: string | null,
  renderKind?: ToolRenderKind,
): ToolRenderKind {
  if (renderKind && renderKind !== 'generic') return renderKind;
  const normalized = normalizeToolName(toolName);
  return toolRenderers.find(renderer => renderer.matches(normalized))?.kind ?? 'generic';
}

export function isFileChangeToolRender(
  toolName?: string | null,
  renderKind?: ToolRenderKind,
): boolean {
  return resolveToolRenderKind(toolName, renderKind) === 'fileChange';
}

export function isBoardOnlyToolRender(
  toolName?: string | null,
  renderKind?: ToolRenderKind,
): boolean {
  const normalized = normalizeToolName(toolName);
  const descriptor = toolRenderers.find(renderer => renderer.matches(normalized));
  return descriptor?.boardOnly === true || resolveToolRenderKind(toolName, renderKind) === 'plan';
}
