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
const companionSource = readFileSync(
  join(process.cwd(), 'src-tauri', 'src', 'companion_window.rs'),
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
const livePositionClamp = companionSource.match(
  /fn keep_current_position_inside_work_area[\s\S]*?\n}\n/,
)?.[0] ?? '';
const placementLoop = companionSource.match(
  /let mut interval = tokio::time::interval[\s\S]*?\n\s*}\);/,
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
assert(
  /should_reapply_configured_anchor/.test(livePositionClamp)
    && /outer_position\(\)/.test(livePositionClamp)
    && /clamped == current/.test(livePositionClamp),
  'the periodic guard must reapply declarative anchors and otherwise preserve live roaming',
);
assert(
  /keep_current_position_inside_work_area/.test(placementLoop)
    && !/place_inside_work_area\(/.test(placementLoop),
  'the periodic guard must clamp roaming in place instead of replaying persisted coordinates',
);

console.log('ok - native window lifecycle preserves application exit and live roaming authority');
