#!/usr/bin/env node

import crypto from 'node:crypto';
import fs from 'node:fs';
import path from 'node:path';

const RUNTIME_VERSION = '4.0.1-nexa.1';
const MODULES = [
  'pptxgenjs', 'image-size', 'jszip', 'lie', 'immediate', 'pako',
  'readable-stream', 'core-util-is', 'inherits', 'isarray',
  'process-nextick-args', 'safe-buffer', 'string_decoder', 'util-deprecate',
  'setimmediate', 'https',
];

function parseArguments(argv) {
  const values = {};
  for (let index = 0; index < argv.length; index += 2) {
    const flag = argv[index];
    const value = argv[index + 1];
    if (!flag?.startsWith('--') || value === undefined) throw new Error('arguments must be --name value pairs');
    values[flag.slice(2)] = value;
  }
  return values;
}

function sha256(file) {
  return crypto.createHash('sha256').update(fs.readFileSync(file)).digest('hex');
}

function inventory(root) {
  const files = [];
  function visit(directory) {
    for (const entry of fs.readdirSync(directory, { withFileTypes: true })) {
      const full = path.join(directory, entry.name);
      if (entry.isSymbolicLink()) throw new Error(`runtime cannot contain symlinks: ${full}`);
      if (entry.isDirectory()) visit(full);
      else if (entry.isFile()) files.push(full);
      else throw new Error(`runtime contains unsupported filesystem entry: ${full}`);
    }
  }
  visit(root);
  files.sort((left, right) => left.localeCompare(right));
  return files.map((file) => ({
    path: path.relative(root, file).split(path.sep).join('/'),
    size: fs.statSync(file).size,
    sha256: sha256(file),
  }));
}

const args = parseArguments(process.argv.slice(2));
if (!args.output) throw new Error('--output is required');
const repository = path.resolve(import.meta.dirname, '..');
const output = path.resolve(args.output);
const nodeSource = path.resolve(args.node ?? process.execPath);
if (!fs.statSync(nodeSource).isFile()) throw new Error(`Node executable is missing: ${nodeSource}`);

fs.mkdirSync(output, { recursive: true });
for (const entry of fs.readdirSync(output)) {
  if (entry === '.gitignore' || entry === 'README.md') continue;
  fs.rmSync(path.join(output, entry), { recursive: true, force: true });
}
if (args['clean-only'] === 'true') {
  process.stdout.write(`${JSON.stringify({ output, cleaned: true })}\n`);
  process.exit(0);
}
fs.mkdirSync(path.join(output, 'node_modules'), { recursive: true });
const nodeName = process.platform === 'win32' ? 'node.exe' : 'node';
fs.copyFileSync(nodeSource, path.join(output, nodeName));
if (process.platform !== 'win32') fs.chmodSync(path.join(output, nodeName), 0o755);

for (const moduleName of MODULES) {
  const source = path.join(repository, 'node_modules', moduleName);
  if (!fs.statSync(source).isDirectory()) throw new Error(`reviewed runtime module is missing: ${moduleName}`);
  fs.cpSync(source, path.join(output, 'node_modules', moduleName), {
    recursive: true,
    dereference: true,
    errorOnExist: true,
  });
}

const files = inventory(output);
const manifest = {
  kind: 'nexaPptxGenJsRuntime',
  manifestVersion: 1,
  runtimeVersion: RUNTIME_VERSION,
  nodeVersion: process.version,
  nodeFile: nodeName,
  moduleRoot: 'node_modules',
  modules: MODULES,
  files,
};
fs.writeFileSync(
  path.join(output, 'runtime-manifest.json'),
  `${JSON.stringify(manifest, null, 2)}\n`,
  'utf8',
);
process.stdout.write(`${JSON.stringify({ output, files: files.length, runtimeVersion: RUNTIME_VERSION })}\n`);
