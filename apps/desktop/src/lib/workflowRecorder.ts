export type WorkflowRecordingStepKind = 'action' | 'decision' | 'check' | 'note';

export interface WorkflowRecordingStep {
  id: string;
  kind: WorkflowRecordingStepKind;
  text: string;
}

export interface RecordedWorkflowPromptInput {
  name: string;
  objective: string;
  context?: string;
  variableInputs?: string[];
  steps: WorkflowRecordingStep[];
  preferences?: string[];
  successCriteria?: string[];
  safetyNotes?: string[];
  replayValues?: string[];
}

const PROMPT_MAX_CHARS = 11_500;

const stepKindLabels: Record<WorkflowRecordingStepKind, string> = {
  action: 'Action',
  decision: 'Decision',
  check: 'Check',
  note: 'Note',
};

export function splitRecordingLines(value: string): string[] {
  return value
    .split(/\r?\n/)
    .map((line) => line.replace(/^[-*]\s+/, '').trim())
    .filter(Boolean);
}

function cleanText(value: string | undefined, fallback: string): string {
  const text = (value ?? '').trim();
  return text.length > 0 ? text : fallback;
}

function uniqueNonEmpty(values: string[] | undefined): string[] {
  const seen = new Set<string>();
  const out: string[] = [];
  for (const value of values ?? []) {
    const normalized = value.trim();
    const key = normalized.toLocaleLowerCase();
    if (!normalized || seen.has(key)) continue;
    seen.add(key);
    out.push(normalized);
  }
  return out;
}

function bulletSection(title: string, values: string[] | undefined): string[] {
  const items = uniqueNonEmpty(values);
  if (items.length === 0) return [];
  return ['', `${title}:`, ...items.map((item) => `- ${item}`)];
}

function stepSection(steps: WorkflowRecordingStep[]): string[] {
  const normalized = steps
    .map((step) => ({
      ...step,
      text: step.text.trim(),
    }))
    .filter((step) => step.text.length > 0);
  if (normalized.length === 0) return [];
  return [
    '',
    'Recorded demonstration:',
    ...normalized.map((step, index) => `${index + 1}. [${stepKindLabels[step.kind]}] ${step.text}`),
  ];
}

function clampPrompt(prompt: string): string {
  if (prompt.length <= PROMPT_MAX_CHARS) return prompt;
  return `${prompt.slice(0, PROMPT_MAX_CHARS - 120).trimEnd()}\n\n[Recording truncated to fit the workflow automation prompt limit.]`;
}

export function buildRecordedWorkflowPrompt(input: RecordedWorkflowPromptInput): string {
  const name = cleanText(input.name, 'Untitled recorded workflow');
  const objective = cleanText(input.objective, 'Replay the recorded workflow and produce the same class of outcome.');
  const context = (input.context ?? '').trim();
  const replayValues = uniqueNonEmpty(input.replayValues);

  const lines = [
    'Replay this recorded workflow as an adaptable Nexa procedure.',
    '',
    `Workflow: ${name}`,
    `Goal: ${objective}`,
    ...(context ? ['', 'Context:', context] : []),
    ...bulletSection('Variable inputs that may change between replays', input.variableInputs),
    ...stepSection(input.steps),
    ...bulletSection('Preferences and decision points to preserve', input.preferences),
    ...bulletSection('Success criteria', input.successCriteria),
    ...bulletSection('Safety and review boundaries', input.safetyNotes),
    ...bulletSection('Runtime values for this replay', replayValues),
    '',
    'Replay contract:',
    '- Adapt the workflow semantically; do not depend on raw cursor positions or exact screen layout.',
    '- Ask for missing required variable values before taking irreversible actions.',
    '- Use the currently available Nexa tools, source scope, browser evidence, and chat context.',
    '- Verify the success criteria before returning the final result.',
    '- Surface assumptions, blocked steps, and evidence gaps instead of silently filling them.',
  ];

  return clampPrompt(lines.join('\n'));
}

export function hasRecordableWorkflow(input: RecordedWorkflowPromptInput): boolean {
  return Boolean(
    input.name.trim()
    && input.objective.trim()
    && input.steps.some((step) => step.text.trim().length > 0),
  );
}
