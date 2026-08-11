// @ts-expect-error The contract runner intentionally omits Node ambient types.
import { readFileSync } from 'node:fs';
// @ts-expect-error The contract runner intentionally omits Node ambient types.
import { join } from 'node:path';

function assert(condition: unknown, message: string): asserts condition {
  if (!condition) throw new Error(message);
}

const mainSource = readFileSync(
  join(process.cwd(), 'src-tauri', 'src', 'main.rs'),
  'utf8',
);

const closePolicy = mainSource.match(
  /fn main_window_close_action[\s\S]*?\n}\n/,
)?.[0] ?? '';
const closeHandler = mainSource.match(
  /tauri::RunEvent::WindowEvent[\s\S]*?\n\s*}\n\s*tauri::RunEvent::Exit/,
)?.[0] ?? '';
const exitOwner = mainSource.match(
  /fn request_application_exit[\s\S]*?\n}\n/,
)?.[0] ?? '';

assert(
  /WindowCloseBehavior::Exit\s*=>\s*MainWindowCloseAction::ExitApplication/.test(closePolicy),
  'direct close must select application exit so the Companion window and tray cannot outlive main',
);
assert(
  /app\.exit\(0\)/.test(exitOwner),
  'the shared application-exit owner must terminate the Tauri process',
);
assert(
  /MainWindowCloseAction::ExitApplication[\s\S]*request_application_exit\(app_handle\)/.test(closeHandler),
  'the main close handler must route direct-exit mode through the shared application owner',
);
assert(
  (mainSource.match(/request_application_exit\(/g) ?? []).length === 3,
  'the title-bar and tray Quit paths must share the same application-exit owner',
);

console.log('ok - native window close owns the full application lifecycle');
