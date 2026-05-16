import { spawnSync } from 'node:child_process';
import { mkdirSync, writeFileSync } from 'node:fs';
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

mkdirSync(outDir, { recursive: true });
writeFileSync(join(outDir, 'package.json'), '{"type":"commonjs"}\n');

run(process.execPath, [
  join(root, 'node_modules', 'typescript', 'bin', 'tsc'),
  '-p',
  join(root, 'tsconfig.streaming-tests.json'),
]);
run(process.execPath, [join(outDir, 'tests', 'streaming-contracts.test.js')]);
