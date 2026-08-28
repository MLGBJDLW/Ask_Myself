import { spawnSync } from 'node:child_process';
import { mkdirSync, readdirSync, rmSync, writeFileSync } from 'node:fs';
import { dirname, join } from 'node:path';
import { fileURLToPath } from 'node:url';

const root = join(dirname(fileURLToPath(import.meta.url)), '..');
const outDir = join(root, 'target', 'streaming-contracts');

function run(command, args) {
  const result = spawnSync(command, args, {
    cwd: root,
    stdio: 'inherit',
    shell: false,
  });
  if (result.status !== 0) {
    process.exit(result.status ?? 1);
  }
}

rmSync(outDir, { recursive: true, force: true });
mkdirSync(outDir, { recursive: true });
writeFileSync(join(outDir, 'package.json'), '{"type":"commonjs"}\n');

run(process.execPath, [
  // Contract tests transpile through the TypeScript compiler API-compatible
  // 6.x bridge, while production type-checking uses the native 7.x `tsc`.
  join(root, 'node_modules', 'typescript', 'bin', 'tsc6'),
  '-p',
  join(root, 'tsconfig.streaming-tests.json'),
]);
for (const file of readdirSync(join(outDir, 'tests')).filter((name) => name.endsWith('.test.js')).sort()) {
  run(process.execPath, [join(outDir, 'tests', file)]);
}
