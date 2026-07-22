import { extractSubtaskArtifacts } from '../src/lib/taskArtifacts';

function assert(condition: unknown, message: string): asserts condition {
  if (!condition) throw new Error(message);
}

const ordinaryRuntimeArtifacts = {
  kind: 'agentTaskArtifacts',
  subtasks: [],
  selectedSkills: {
    kind: 'selectedSkills',
    skills: [
      { id: 'skill-docs', name: 'Docs', enabled: true },
    ],
  },
  workflow: {
    runs: [
      { id: 'workflow-1', label: 'Build report', status: 'done' },
    ],
  },
  tools: [
    { id: 'tool-1', label: 'Search files', status: 'done' },
  ],
};

assert(
  extractSubtaskArtifacts(ordinaryRuntimeArtifacts).length === 0,
  'skills, workflows, and ordinary tools must not be projected as subagents',
);

const realSubtasks = extractSubtaskArtifacts({
  kind: 'agentTaskArtifacts',
  subtasks: [
    {
      id: 'subtask-1',
      parentRunId: 'run-1',
      label: 'Research implementation',
      role: 'researcher',
      status: 'completed',
    },
  ],
});
assert(realSubtasks.length === 1, 'canonical subtask rows should remain visible');
assert(realSubtasks[0].id === 'subtask-1', 'real subtask identity should be preserved');

console.log('ok - task artifacts require explicit subagent provenance');
