import { resolveFileBadgeIcon } from '../src/components/ui/fileBadgeCatalog';

function equal(actual: unknown, expected: unknown, label: string): void {
  if (actual !== expected) {
    throw new Error(`${label}: expected ${String(expected)}, received ${String(actual)}`);
  }
}

const expectedTreatments: Record<string, string> = {
  'server.py': 'brand-accent',
  'main.ts': 'brand-accent',
  'component.tsx': 'brand-accent',
  'index.js': 'brand-accent',
  'lib.rs': 'brand-accent',
  'main.go': 'brand-accent',
  'Main.java': 'brand-accent',
  'Main.kt': 'brand-accent',
  'App.swift': 'brand-accent',
  Dockerfile: 'brand-accent',
};

for (const [filename, treatment] of Object.entries(expectedTreatments)) {
  equal(resolveFileBadgeIcon(filename).treatment, treatment, `${filename} treatment`);
}

equal(resolveFileBadgeIcon('notes.txt').treatment, 'mono', 'unknown families keep mono fallback');
equal(resolveFileBadgeIcon('server.py').brandColor, '#3776AB', 'Python primary brand color');
equal(resolveFileBadgeIcon('server.py').accentColor, '#FFD43B', 'Python secondary accent');

console.log('ok - file badge visuals separate brand treatment from shell tone');
