// @ts-expect-error The contract runner intentionally omits Node ambient types.
import { readFileSync } from 'node:fs';
// @ts-expect-error The contract runner intentionally omits Node ambient types.
import { join } from 'node:path';

function assert(condition: unknown, message: string): asserts condition {
  if (!condition) throw new Error(message);
}

const root = process.cwd();
const workflowsPage = readFileSync(join(root, 'src/pages/WorkflowsPage.tsx'), 'utf8');
const workflowCommands = readFileSync(join(root, 'src-tauri/src/commands/workflows.rs'), 'utf8');
const agentChat = readFileSync(join(root, 'src-tauri/src/commands/agent_chat.rs'), 'utf8');
const runHandler = workflowsPage.slice(
  workflowsPage.indexOf('const runAutomation'),
  workflowsPage.indexOf('const runDueAutomation'),
);
const dueHandler = workflowsPage.slice(
  workflowsPage.indexOf('const runDueAutomation'),
  workflowsPage.indexOf('const editAutomation'),
);

assert(
  dueHandler.includes('api.startDueWorkflowAutomationRun'),
  'Due Now must invoke the authoritative backend scheduled-launch command',
);
assert(
  !dueHandler.includes('api.queueDueWorkflowAutomationDelivery'),
  'Due Now must not claim an occurrence and hand it to unrestricted generic Chat',
);
assert(
  dueHandler.includes("navigate(`/chat/${launch.conversationId}`)"),
  'Due Now must open the already-launched authoritative run conversation',
);
assert(
  runHandler.includes('api.startWorkflowAutomationRun')
    && !runHandler.includes('api.queueWorkflowAutomationDelivery'),
  'Run now must not bypass a scheduled definition policy through generic Chat',
);

const startDueCommand = workflowCommands.slice(
  workflowCommands.indexOf('pub async fn start_due_workflow_automation_run_cmd'),
  workflowCommands.indexOf('pub fn init_task_orchestrator_scheduler'),
);
const schedulerTick = workflowCommands.slice(
  workflowCommands.indexOf('pub async fn run_task_orchestrator_scheduler_tick'),
  workflowCommands.indexOf('pub async fn record_workflow_automation_run_cmd'),
);
const scheduledLaunchSeam = workflowCommands.slice(
  workflowCommands.indexOf('async fn launch_authoritative_scheduled_workflow'),
  workflowCommands.indexOf('pub async fn save_workflow_automation_cmd'),
);
assert(
  startDueCommand.includes('launch_authoritative_scheduled_workflow'),
  'public due-start must use the same authoritative launch seam as the scheduler',
);
assert(
  schedulerTick.includes('launch_authoritative_scheduled_workflow'),
  'background scheduler must use the authoritative scheduled launch seam',
);
assert(
  !schedulerTick.includes('.take(') && schedulerTick.includes('launches.len() >= launch_limit'),
  'skips and retry backoff must not consume the scheduler successful-launch budget',
);
assert(
  agentChat.includes('capability_registry_may_select_text_route')
    && agentChat.includes('agent_config_override_is_authoritative'),
  'scheduled route snapshots must remain authoritative over Capability Registry rerouting',
);
assert(
  scheduledLaunchSeam.includes('pause_workflow_automation_for_approval_once'),
  'approval pauses must use the atomic single-winner core transaction',
);
assert(
  workflowCommands.includes('legacy_workflow_schedule_config'),
  'omitted legacy scheduleConfig must pass through the safe legacy classifier',
);

console.log('ok - scheduled workflow launch contracts');
