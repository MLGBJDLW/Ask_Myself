import { spawn } from 'node:child_process';
import { fileURLToPath } from 'node:url';
import path from 'node:path';

const __dirname = path.dirname(fileURLToPath(import.meta.url));
const appDir = path.resolve(__dirname, '..');
const args = process.argv.slice(2);

const command = args[0];
const isDevCommand = command === 'dev';
const script = isDevCommand
  ? path.join(appDir, 'scripts', 'tauri-dev-autoport.mjs')
  : null;
const tauriCli = path.join(appDir, 'node_modules', '@tauri-apps', 'cli', 'tauri.js');
const executable = process.execPath;
const childArgs = isDevCommand
  ? [script, ...args.slice(1)]
  : [tauriCli, ...args];
const child = spawn(executable, childArgs, {
  cwd: appDir,
  stdio: 'inherit',
  env: process.env,
});

child.on('exit', (code, signal) => {
  if (signal) {
    process.kill(process.pid, signal);
    return;
  }
  process.exit(code ?? 0);
});

child.on('error', (error) => {
  console.error('[tauri-cli] Failed to start Tauri command:', error);
  process.exit(1);
});
