import type { ElementType } from 'react';
import {
  Binary,
  BookText,
  Braces,
  Database,
  FileAudio,
  FileCode2,
  FileCog,
  FileImage,
  FileTerminal,
  FileText,
  FileType,
  FileVideo,
  FolderOpen,
  Hash,
  NotebookText,
  Package,
  Presentation,
} from 'lucide-react';
import {
  FaFileAudio,
  FaFileCsv,
  FaFileExcel,
  FaFileImage,
  FaFilePdf,
  FaFilePowerpoint,
  FaFileVideo,
  FaFileWord,
  FaFileZipper,
} from 'react-icons/fa6';
import {
  SiAstro,
  SiBun,
  SiC,
  SiClojure,
  SiCplusplus,
  SiCss,
  SiDart,
  SiDeno,
  SiDocker,
  SiDotnet,
  SiElixir,
  SiGit,
  SiGitignoredotio,
  SiGnubash,
  SiGo,
  SiGraphql,
  SiHaskell,
  SiHtml5,
  SiJavascript,
  SiJest,
  SiJson,
  SiJupyter,
  SiKotlin,
  SiLua,
  SiMarkdown,
  SiNpm,
  SiOpenjdk,
  SiPerl,
  SiPhp,
  SiPnpm,
  SiPython,
  SiReact,
  SiRuby,
  SiRust,
  SiSass,
  SiScala,
  SiSolidity,
  SiSvelte,
  SiSwift,
  SiTerraform,
  SiTypescript,
  SiVite,
  SiVitest,
  SiVuedotjs,
  SiYaml,
  SiYarn,
  SiZig,
} from 'react-icons/si';

export type FileBadgeTone =
  | 'red'
  | 'rose'
  | 'blue'
  | 'sky'
  | 'cyan'
  | 'teal'
  | 'green'
  | 'emerald'
  | 'orange'
  | 'amber'
  | 'yellow'
  | 'purple'
  | 'fuchsia'
  | 'pink'
  | 'violet'
  | 'indigo'
  | 'slate'
  | 'gray';

export interface FileBadgeIconStyle {
  tone: FileBadgeTone;
  Icon: ElementType;
  iconId: string;
}

function style(tone: FileBadgeTone, Icon: ElementType, iconId: string): FileBadgeIconStyle {
  return { tone, Icon, iconId };
}

const extStyles: Record<string, FileBadgeIconStyle> = {
  // Documents and publishing
  '.pdf': style('red', FaFilePdf, 'pdf'),
  '.doc': style('blue', FaFileWord, 'word'),
  '.docx': style('blue', FaFileWord, 'word'),
  '.rtf': style('sky', FileText, 'rich-text'),
  '.odt': style('sky', FaFileWord, 'open-document-text'),
  '.epub': style('teal', BookText, 'ebook'),
  '.mobi': style('teal', BookText, 'ebook'),

  // Spreadsheets and tabular data
  '.xlsx': style('green', FaFileExcel, 'excel'),
  '.xls': style('green', FaFileExcel, 'excel'),
  '.xlsm': style('green', FaFileExcel, 'excel'),
  '.ods': style('green', FaFileExcel, 'open-document-sheet'),
  '.csv': style('emerald', FaFileCsv, 'csv'),
  '.tsv': style('emerald', FaFileCsv, 'tsv'),

  // Presentations
  '.pptx': style('orange', FaFilePowerpoint, 'powerpoint'),
  '.ppt': style('orange', FaFilePowerpoint, 'powerpoint'),
  '.odp': style('orange', Presentation, 'open-document-presentation'),

  // Prose and notebooks
  '.md': style('cyan', SiMarkdown, 'markdown'),
  '.markdown': style('cyan', SiMarkdown, 'markdown'),
  '.mdx': style('cyan', SiMarkdown, 'mdx'),
  '.rst': style('teal', BookText, 'restructured-text'),
  '.org': style('teal', BookText, 'org-mode'),
  '.txt': style('gray', FileText, 'text'),
  '.log': style('amber', FileText, 'log'),
  '.ipynb': style('orange', SiJupyter, 'jupyter'),

  // Languages
  '.py': style('yellow', SiPython, 'python'),
  '.pyw': style('yellow', SiPython, 'python'),
  '.pyi': style('yellow', SiPython, 'python'),
  '.ts': style('blue', SiTypescript, 'typescript'),
  '.tsx': style('cyan', SiReact, 'react-typescript'),
  '.js': style('amber', SiJavascript, 'javascript'),
  '.jsx': style('cyan', SiReact, 'react-javascript'),
  '.mjs': style('amber', SiJavascript, 'javascript-module'),
  '.cjs': style('amber', SiJavascript, 'javascript-commonjs'),
  '.rs': style('orange', SiRust, 'rust'),
  '.go': style('cyan', SiGo, 'go'),
  '.java': style('red', SiOpenjdk, 'java'),
  '.jar': style('red', SiOpenjdk, 'java-archive'),
  '.kt': style('purple', SiKotlin, 'kotlin'),
  '.kts': style('purple', SiKotlin, 'kotlin-script'),
  '.swift': style('orange', SiSwift, 'swift'),
  '.c': style('indigo', SiC, 'c'),
  '.h': style('indigo', SiC, 'c-header'),
  '.cc': style('indigo', SiCplusplus, 'cpp'),
  '.cpp': style('indigo', SiCplusplus, 'cpp'),
  '.cxx': style('indigo', SiCplusplus, 'cpp'),
  '.hpp': style('indigo', SiCplusplus, 'cpp-header'),
  '.cs': style('violet', SiDotnet, 'dotnet'),
  '.fs': style('violet', SiDotnet, 'fsharp'),
  '.fsx': style('violet', SiDotnet, 'fsharp-script'),
  '.rb': style('rose', SiRuby, 'ruby'),
  '.php': style('indigo', SiPhp, 'php'),
  '.phtml': style('indigo', SiPhp, 'php'),
  '.lua': style('blue', SiLua, 'lua'),
  '.dart': style('sky', SiDart, 'dart'),
  '.scala': style('red', SiScala, 'scala'),
  '.sc': style('red', SiScala, 'scala-script'),
  '.hs': style('purple', SiHaskell, 'haskell'),
  '.lhs': style('purple', SiHaskell, 'literate-haskell'),
  '.ex': style('purple', SiElixir, 'elixir'),
  '.exs': style('purple', SiElixir, 'elixir-script'),
  '.clj': style('green', SiClojure, 'clojure'),
  '.cljs': style('green', SiClojure, 'clojurescript'),
  '.cljc': style('green', SiClojure, 'clojure-common'),
  '.pl': style('blue', SiPerl, 'perl'),
  '.pm': style('blue', SiPerl, 'perl-module'),
  '.zig': style('amber', SiZig, 'zig'),
  '.sol': style('slate', SiSolidity, 'solidity'),
  '.r': style('sky', FileCode2, 'r-language'),
  '.sql': style('cyan', Database, 'sql'),

  // Shells and executable scripts
  '.sh': style('emerald', SiGnubash, 'shell'),
  '.bash': style('emerald', SiGnubash, 'bash'),
  '.zsh': style('emerald', SiGnubash, 'zsh'),
  '.fish': style('emerald', FileTerminal, 'fish-shell'),
  '.ps1': style('blue', FileTerminal, 'powershell'),
  '.bat': style('slate', FileTerminal, 'batch'),
  '.cmd': style('slate', FileTerminal, 'command-script'),

  // Web, styles, configuration, and infrastructure
  '.html': style('orange', SiHtml5, 'html'),
  '.htm': style('orange', SiHtml5, 'html'),
  '.css': style('blue', SiCss, 'css'),
  '.scss': style('pink', SiSass, 'sass'),
  '.sass': style('pink', SiSass, 'sass'),
  '.less': style('blue', Hash, 'less'),
  '.vue': style('emerald', SiVuedotjs, 'vue'),
  '.svelte': style('orange', SiSvelte, 'svelte'),
  '.astro': style('purple', SiAstro, 'astro'),
  '.json': style('yellow', SiJson, 'json'),
  '.jsonl': style('yellow', SiJson, 'json-lines'),
  '.yaml': style('purple', SiYaml, 'yaml'),
  '.yml': style('purple', SiYaml, 'yaml'),
  '.toml': style('purple', FileCog, 'toml'),
  '.xml': style('orange', Braces, 'xml'),
  '.ini': style('slate', FileCog, 'ini'),
  '.conf': style('slate', FileCog, 'config'),
  '.config': style('slate', FileCog, 'config'),
  '.lock': style('slate', Package, 'lockfile'),
  '.graphql': style('pink', SiGraphql, 'graphql'),
  '.gql': style('pink', SiGraphql, 'graphql'),
  '.tf': style('violet', SiTerraform, 'terraform'),
  '.tfvars': style('violet', SiTerraform, 'terraform-vars'),
  '.proto': style('blue', Braces, 'protobuf'),

  // Images, diagrams, and fonts
  '.jpg': style('yellow', FaFileImage, 'image'),
  '.jpeg': style('yellow', FaFileImage, 'image'),
  '.png': style('cyan', FaFileImage, 'image'),
  '.gif': style('pink', FaFileImage, 'image'),
  '.webp': style('teal', FaFileImage, 'image'),
  '.svg': style('orange', FaFileImage, 'svg'),
  '.bmp': style('sky', FileImage, 'bitmap'),
  '.tiff': style('sky', FileImage, 'tiff'),
  '.tif': style('sky', FileImage, 'tiff'),
  '.ico': style('violet', FileImage, 'icon'),
  '.drawio': style('orange', FileImage, 'diagram'),
  '.ttf': style('slate', FileType, 'font'),
  '.otf': style('slate', FileType, 'font'),
  '.woff': style('slate', FileType, 'webfont'),
  '.woff2': style('slate', FileType, 'webfont'),

  // Archives and binaries
  '.zip': style('slate', FaFileZipper, 'archive'),
  '.tar': style('slate', FaFileZipper, 'archive'),
  '.gz': style('slate', FaFileZipper, 'archive'),
  '.tgz': style('slate', FaFileZipper, 'archive'),
  '.bz2': style('slate', FaFileZipper, 'archive'),
  '.xz': style('slate', FaFileZipper, 'archive'),
  '.zst': style('slate', FaFileZipper, 'archive'),
  '.7z': style('slate', FaFileZipper, 'archive'),
  '.rar': style('slate', FaFileZipper, 'archive'),
  '.cab': style('slate', FaFileZipper, 'archive'),
  '.iso': style('slate', FaFileZipper, 'disk-image'),
  '.exe': style('gray', Binary, 'binary'),
  '.dll': style('gray', Binary, 'binary'),
  '.so': style('gray', Binary, 'binary'),
  '.dylib': style('gray', Binary, 'binary'),
  '.bin': style('gray', Binary, 'binary'),
  '.wasm': style('purple', Binary, 'webassembly'),
  '.class': style('red', Binary, 'java-bytecode'),
  '.db': style('cyan', Database, 'database'),
  '.sqlite': style('cyan', Database, 'sqlite'),
  '.sqlite3': style('cyan', Database, 'sqlite'),

  // Video and audio
  '.mp4': style('violet', FaFileVideo, 'video'),
  '.mkv': style('violet', FaFileVideo, 'video'),
  '.webm': style('violet', FaFileVideo, 'video'),
  '.mov': style('violet', FaFileVideo, 'video'),
  '.avi': style('violet', FileVideo, 'video'),
  '.flv': style('violet', FileVideo, 'video'),
  '.wmv': style('violet', FileVideo, 'video'),
  '.m4v': style('violet', FileVideo, 'video'),
  '.mpeg': style('violet', FileVideo, 'video'),
  '.mpg': style('violet', FileVideo, 'video'),
  '.mp3': style('fuchsia', FaFileAudio, 'audio'),
  '.wav': style('fuchsia', FaFileAudio, 'audio'),
  '.flac': style('fuchsia', FaFileAudio, 'audio'),
  '.ogg': style('fuchsia', FaFileAudio, 'audio'),
  '.aac': style('fuchsia', FileAudio, 'audio'),
  '.m4a': style('fuchsia', FileAudio, 'audio'),
  '.wma': style('fuchsia', FileAudio, 'audio'),
  '.opus': style('fuchsia', FileAudio, 'audio'),
};

const namedFileStyles: Record<string, FileBadgeIconStyle> = {
  dockerfile: style('blue', SiDocker, 'docker'),
  makefile: style('slate', FileTerminal, 'makefile'),
  justfile: style('slate', FileTerminal, 'justfile'),
  license: style('green', BookText, 'license'),
  copying: style('green', BookText, 'license'),
  readme: style('cyan', NotebookText, 'readme'),
  'package.json': style('red', SiNpm, 'npm'),
  'package-lock.json': style('red', SiNpm, 'npm-lock'),
  'pnpm-lock.yaml': style('amber', SiPnpm, 'pnpm-lock'),
  'yarn.lock': style('blue', SiYarn, 'yarn-lock'),
  'bun.lock': style('amber', SiBun, 'bun-lock'),
  'bun.lockb': style('amber', SiBun, 'bun-lock'),
  'deno.json': style('slate', SiDeno, 'deno'),
  'deno.jsonc': style('slate', SiDeno, 'deno'),
  '.gitignore': style('orange', SiGitignoredotio, 'gitignore'),
  '.gitattributes': style('orange', SiGit, 'git'),
  '.gitmodules': style('orange', SiGit, 'git'),
};

const defaultStyle = style('gray', FileType, 'file');
const directoryStyle = style('gray', FolderOpen, 'folder');

function patternStyle(lower: string): FileBadgeIconStyle | undefined {
  if (lower.startsWith('.env')) return style('green', FileCog, 'environment');
  if (/^(docker-compose|compose)(\.[^.]+)?\.ya?ml$/.test(lower)) {
    return style('blue', SiDocker, 'docker-compose');
  }
  if (/^vite\.config\./.test(lower)) return style('violet', SiVite, 'vite');
  if (/^vitest(?:\.[^.]+)?\.config\./.test(lower)) return style('green', SiVitest, 'vitest');
  if (/^jest\.config\./.test(lower)) return style('rose', SiJest, 'jest');
  if (/^tsconfig(?:\.[^.]+)?\.json$/.test(lower)) {
    return style('blue', SiTypescript, 'typescript-config');
  }
  return undefined;
}

export function resolveFileBadgeIcon(filename: string, isDirectory = false): FileBadgeIconStyle {
  if (isDirectory) return directoryStyle;

  const lower = filename.toLowerCase();
  const namedStyle = namedFileStyles[lower] ?? namedFileStyles[lower.split('.')[0]];
  if (namedStyle) return namedStyle;

  const matchedPattern = patternStyle(lower);
  if (matchedPattern) return matchedPattern;

  const dot = lower.lastIndexOf('.');
  if (dot === -1) return defaultStyle;
  return extStyles[lower.slice(dot)] ?? defaultStyle;
}
