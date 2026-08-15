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
const tauriConfig = JSON.parse(readFileSync(
  join(process.cwd(), 'src-tauri', 'tauri.conf.json'),
  'utf8',
)) as {
  app?: { windows?: Array<{ visible?: boolean; backgroundColor?: string }> };
};
const indexHtml = readFileSync(join(process.cwd(), 'index.html'), 'utf8');
const bootstrapSource = readFileSync(join(process.cwd(), 'src', 'main.tsx'), 'utf8');
const themeSource = readFileSync(join(process.cwd(), 'src', 'lib', 'theme.ts'), 'utf8');
const mainCapability = JSON.parse(readFileSync(
  join(process.cwd(), 'src-tauri', 'capabilities', 'default.json'),
  'utf8',
)) as { permissions?: string[] };

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
const mainWindow = tauriConfig.app?.windows?.[0];
assert(
  mainWindow?.visible === false,
  'the native main window must stay hidden until the branded startup surface has painted',
);
assert(
  /StateFlags::all\(\)\s*&\s*!\(StateFlags::DECORATIONS\s*\|\s*StateFlags::VISIBLE\)/.test(mainSource),
  'persisted window state must not restore visibility before the branded startup surface paints',
);
assert(
  typeof mainWindow.backgroundColor === 'string' && mainWindow.backgroundColor !== '#ffffff',
  'the native webview background must match the branded startup surface instead of flashing white',
);
assert(
  /data-testid=["']startup-splash["']/.test(indexHtml)
    && /logo-small\.svg/.test(indexHtml)
    && /prefers-reduced-motion/.test(indexHtml),
  'the first HTML paint must contain a branded, reduced-motion-safe startup animation',
);
assert(
  /name=["']color-scheme["']\s+content=["']dark light["']/.test(indexHtml)
    && /root\.style\.colorScheme\s*=\s*isLightTheme\(theme\)\s*\?\s*["']light["']\s*:\s*["']dark["']/.test(themeSource),
  'native controls must follow the active light or dark application theme after startup',
);
assert(
  /requestAnimationFrame/.test(bootstrapSource)
    && /getCurrentWindow\(\)/.test(bootstrapSource)
    && /\.show\(\)/.test(bootstrapSource)
    && /\.setFocus\(\)/.test(bootstrapSource),
  'the bootstrap must reveal the correctly sized native window only after the startup surface paints',
);
assert(
  mainCapability.permissions?.includes('core:window:allow-show')
    && mainCapability.permissions.includes('core:window:allow-set-focus'),
  'the main webview must be authorized to reveal and focus the initially hidden native window',
);

console.log('ok - native window lifecycle preserves startup, application exit, and live roaming authority');
