import fs from 'node:fs';
import path from 'node:path';
import { fileURLToPath } from 'node:url';

const repositoryRoot = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '..');
const lockPath = path.join(repositoryRoot, 'Cargo.lock');

function cargoPackageVersion(manifestPath) {
  const manifest = fs.readFileSync(path.join(repositoryRoot, manifestPath), 'utf8');
  const packageHeader = /^\[package\]\s*$/mu.exec(manifest);
  if (!packageHeader) {
    throw new Error(`Unable to resolve [package] from ${manifestPath}`);
  }
  const packageRemainder = manifest.slice(packageHeader.index + packageHeader[0].length);
  const nextSection = /^\[/mu.exec(packageRemainder);
  const packageSection = packageRemainder.slice(0, nextSection?.index);
  const version = packageSection?.match(/^version\s*=\s*"([^"]+)"\s*$/mu)?.[1];
  if (!version) {
    throw new Error(`Unable to resolve [package].version from ${manifestPath}`);
  }
  return version;
}

export function synchronizePackageVersion(lockContents, packageName, version) {
  let matches = 0;
  const synchronized = lockContents.replace(
    /^\[\[package\]\]\s*$([\s\S]*?)(?=^\[\[package\]\]\s*$|(?![\s\S]))/gmu,
    (packageBlock) => {
      const name = packageBlock.match(/^name\s*=\s*"([^"]+)"\s*$/mu)?.[1];
      if (name !== packageName) {
        return packageBlock;
      }
      matches += 1;
      if (!/^version\s*=\s*"[^"]+"\s*$/mu.test(packageBlock)) {
        throw new Error(`Cargo.lock package ${packageName} has no version field`);
      }
      return packageBlock.replace(
        /^version\s*=\s*"[^"]+"\s*$/mu,
        `version = "${version}"`,
      );
    },
  );
  if (matches !== 1) {
    throw new Error(`Expected one Cargo.lock package named ${packageName}; found ${matches}`);
  }
  return synchronized;
}

export function expectedWorkspaceLock(lockContents) {
  const packages = [
    ['nexa-core', 'crates/core/Cargo.toml'],
    ['nexa-desktop', 'apps/desktop/src-tauri/Cargo.toml'],
  ];
  return packages.reduce(
    (contents, [packageName, manifestPath]) =>
      synchronizePackageVersion(contents, packageName, cargoPackageVersion(manifestPath)),
    lockContents,
  );
}

const isCli =
  process.argv[1] &&
  path.resolve(process.argv[1]) === path.resolve(fileURLToPath(import.meta.url));

if (isCli) {
  const lockContents = fs.readFileSync(lockPath, 'utf8');
  const expected = expectedWorkspaceLock(lockContents);
  if (expected !== lockContents) {
    if (process.argv.includes('--write')) {
      fs.writeFileSync(lockPath, expected, 'utf8');
      process.stdout.write('updated Cargo.lock workspace package versions\n');
    } else {
      process.stderr.write(
        'Cargo.lock workspace package versions are stale; run this script with --write\n',
      );
      process.exitCode = 1;
    }
  } else {
    process.stdout.write('Cargo.lock workspace package versions are synchronized\n');
  }
}
