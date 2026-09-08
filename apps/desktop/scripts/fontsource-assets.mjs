import { readFileSync } from 'node:fs';
import { join } from 'node:path';
import { fileURLToPath } from 'node:url';

/** Keep the original licenses beside offline assets and omit legacy WOFF duplicates. */
export function fontsourceAssets() {
  const root = fileURLToPath(new URL('../', import.meta.url));
  return {
    name: 'nexa-fontsource-assets',
    enforce: 'pre',
    transform(code, id) {
      if (!id.includes('@fontsource') || !id.endsWith('.css')) return null;
      return code.replace(/,\s*url\([^)]*\.woff\)\s*format\(['"]woff['"]\)/g, '');
    },
    generateBundle() {
      const pkg = JSON.parse(readFileSync(join(root, 'package.json'), 'utf8'));
      for (const name of Object.keys(pkg.dependencies).filter(name => name.startsWith('@fontsource'))) {
        this.emitFile({ type: 'asset', fileName: `font-licenses/${name.replace('@', '').replace('/', '-')}.txt`, source: readFileSync(join(root, 'node_modules', name, 'LICENSE'), 'utf8') });
      }
    },
  };
}
