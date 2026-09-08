export interface FontPreset {
  id: string;
  name: string;
  family: string;
  kind: 'cjk' | 'text' | 'mono';
  load: () => Promise<unknown>;
}

// Only the selected font's CSS and Unicode subsets are loaded into the WebView.
// Vite packages WOFF2 plus each family's original license for offline use.
export const FONT_PRESETS: FontPreset[] = [
  { id: 'noto-sans-sc', name: '思源黑体 · Noto Sans SC', family: 'Noto Sans SC', kind: 'cjk', load: () => import('@fontsource/noto-sans-sc/400.css') },
  { id: 'noto-serif-sc', name: '思源宋体 · Noto Serif SC', family: 'Noto Serif SC', kind: 'cjk', load: () => import('@fontsource/noto-serif-sc/400.css') },
  { id: 'lxgw-wenkai-tc', name: '霞鹜文楷 · LXGW WenKai', family: 'LXGW WenKai TC', kind: 'cjk', load: () => import('@fontsource/lxgw-wenkai-tc/400.css') },
  { id: 'zcool-xiaowei', name: '站酷小薇 · ZCOOL XiaoWei', family: 'ZCOOL XiaoWei', kind: 'cjk', load: () => import('@fontsource/zcool-xiaowei/400.css') },
  { id: 'zcool-qingke-huangyou', name: '站酷庆科黄油 · ZCOOL QingKe', family: 'ZCOOL QingKe HuangYou', kind: 'cjk', load: () => import('@fontsource/zcool-qingke-huangyou/400.css') },
  { id: 'ma-shan-zheng', name: '马善政楷书 · Ma Shan Zheng', family: 'Ma Shan Zheng', kind: 'cjk', load: () => import('@fontsource/ma-shan-zheng/400.css') },
  { id: 'inter', name: 'Inter', family: 'Inter Variable', kind: 'text', load: () => import('@fontsource-variable/inter') },
  { id: 'source-sans-3', name: 'Source Sans 3', family: 'Source Sans 3', kind: 'text', load: () => import('@fontsource/source-sans-3/400.css') },
  { id: 'lato', name: 'Lato', family: 'Lato', kind: 'text', load: () => import('@fontsource/lato/400.css') },
  { id: 'ibm-plex-sans', name: 'IBM Plex Sans', family: 'IBM Plex Sans', kind: 'text', load: () => import('@fontsource/ibm-plex-sans/400.css') },
  { id: 'source-serif-4', name: 'Source Serif 4', family: 'Source Serif 4', kind: 'text', load: () => import('@fontsource/source-serif-4/400.css') },
  { id: 'literata', name: 'Literata', family: 'Literata', kind: 'text', load: () => import('@fontsource/literata/400.css') },
  { id: 'jetbrains-mono', name: 'JetBrains Mono', family: 'JetBrains Mono', kind: 'mono', load: () => import('@fontsource/jetbrains-mono/400.css') },
  { id: 'fira-code', name: 'Fira Code', family: 'Fira Code', kind: 'mono', load: () => import('@fontsource/fira-code/400.css') },
  { id: 'source-code-pro', name: 'Source Code Pro', family: 'Source Code Pro', kind: 'mono', load: () => import('@fontsource/source-code-pro/400.css') },
  { id: 'ibm-plex-mono', name: 'IBM Plex Mono', family: 'IBM Plex Mono', kind: 'mono', load: () => import('@fontsource/ibm-plex-mono/400.css') },
];
