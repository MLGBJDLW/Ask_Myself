export function isWebUrl(value: string | undefined | null): boolean {
  if (!value) return false;
  return /^https?:\/\//i.test(value.trim());
}

export function sourceHost(value: string | undefined | null): string {
  if (!isWebUrl(value)) return '';
  const urlValue = value ?? '';
  try {
    return new URL(urlValue).hostname.replace(/^www\./i, '');
  } catch {
    return urlValue;
  }
}

export function sourceBasename(path: string | undefined | null): string {
  if (!path) return '';
  if (isWebUrl(path)) {
    const host = sourceHost(path);
    try {
      const url = new URL(path);
      const tail = url.pathname.split('/').filter(Boolean).pop();
      return tail ? `${host}/${decodeURIComponent(tail)}` : host;
    } catch {
      return host || path;
    }
  }
  const normalized = path.replace(/[\\/]+$/, '');
  const lastSep = Math.max(normalized.lastIndexOf('/'), normalized.lastIndexOf('\\'));
  return lastSep === -1 ? normalized : normalized.slice(lastSep + 1);
}

export function sourceDirectory(path: string | undefined | null): string {
  if (!path) return '';
  if (isWebUrl(path)) return sourceHost(path);
  const parts = path.replace(/\\/g, '/').split('/');
  parts.pop();
  return parts.join('/') || '/';
}

export function sourceKindLabel(path: string | undefined | null): string {
  return isWebUrl(path) ? 'WEB' : 'FILE';
}
