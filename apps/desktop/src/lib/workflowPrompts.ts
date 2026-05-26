export interface WorkflowPromptTemplate {
  id: string;
  promptTemplate?: string;
}

export function buildWorkflowBatchPrompt(template: WorkflowPromptTemplate, batchGoal?: string): string {
  const goal = (batchGoal ?? template.promptTemplate ?? '').trim();
  const goalBlock = goal || 'Describe the goal or material for this workflow here.';

  return [
    'Use spawn_subagent_batch for this workflow template.',
    '',
    `workflow_template: ${template.id}`,
    'batch_goal:',
    goalBlock,
    '',
    'Call spawn_subagent_batch with the workflow_template and batch_goal above before drafting the final response.',
  ].join('\n');
}
