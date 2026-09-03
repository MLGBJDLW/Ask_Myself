import { execFileSync } from 'node:child_process';
import { readFileSync } from 'node:fs';
import { dirname, join } from 'node:path';
import { fileURLToPath } from 'node:url';

const scriptsDir = dirname(fileURLToPath(import.meta.url));
const desktopRoot = dirname(scriptsDir);

function readPackage(...segments) {
  return JSON.parse(readFileSync(join(desktopRoot, 'node_modules', ...segments, 'package.json'), 'utf8'));
}

function fail(message) {
  process.stderr.write(`TypeScript runtime contract failed: ${message}\n`);
  process.exit(1);
}

const native = readPackage('@typescript', 'native');
const compatibility = readPackage('typescript');

if (native.name !== 'typescript' || !/^7\.\d+\.\d+$/.test(native.version)) {
  fail(`@typescript/native must resolve to stable TypeScript 7.x; received ${native.name}@${native.version}`);
}
if (compatibility.name !== '@typescript/typescript6' || !/^6\.\d+\.\d+$/.test(compatibility.version)) {
  fail(`the compiler API bridge must remain @typescript/typescript6; received ${compatibility.name}@${compatibility.version}`);
}

const nativeCli = join(desktopRoot, 'node_modules', '@typescript', 'native', 'bin', 'tsc');
const reportedVersion = execFileSync(process.execPath, [nativeCli, '--version'], {
  encoding: 'utf8',
}).trim();
if (reportedVersion !== `Version ${native.version}`) {
  fail(`tsc resolved to ${reportedVersion || 'an unknown compiler'} instead of Version ${native.version}`);
}

process.stdout.write(
  `TypeScript runtime ok: native tsc ${native.version}; compiler API bridge ${compatibility.version}\n`,
);
