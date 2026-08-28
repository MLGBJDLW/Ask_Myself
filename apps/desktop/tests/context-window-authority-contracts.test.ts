// @ts-expect-error The contract runner intentionally omits Node ambient types.
import { readFileSync } from 'node:fs';
// @ts-expect-error The contract runner intentionally omits Node ambient types.
import { join } from 'node:path';

function assert(condition: unknown, message: string): asserts condition {
  if (!condition) throw new Error(message);
}

const root = process.cwd();
const api = readFileSync(join(root, 'src/lib/api.ts'), 'utf8');
const agentChat = readFileSync(join(root, 'src-tauri/src/commands/agent_chat.rs'), 'utf8');
const desktopSession = readFileSync(join(root, 'src-tauri/src/desktop_agent_session.rs'), 'utf8');
const subagentTool = readFileSync(join(root, 'src-tauri/src/subagent_tool.rs'), 'utf8');
const main = readFileSync(join(root, 'src-tauri/src/main.rs'), 'utf8');

assert(
  !api.includes("invoke<number>('get_model_context_window'"),
  'the model-only scalar context-window IPC must remain retired',
);
assert(
  !agentChat.includes('pub fn get_model_context_window('),
  'the backend must not expose a model-only scalar context-window command',
);
assert(
  !main.includes('commands::get_model_context_window,'),
  'the scalar context-window command must not be registered with Tauri',
);
assert(
  agentChat.includes('resolve_endpoint_model_context_window('),
  'the route-aware IPC must resolve context through the core catalog interface',
);
assert(
  desktopSession.includes('resolve_endpoint_model_context_window(')
    && subagentTool.includes('resolve_endpoint_model_context_window('),
  'desktop and delegated workers must share the core endpoint context resolver',
);
assert(
  !desktopSession.includes('fn resolve_endpoint_context_window(')
    && !subagentTool.includes('fn endpoint_scoped_context_window('),
  'desktop adapters must not restore private copies of endpoint context resolution',
);

console.log('ok - context-window authority contracts');
