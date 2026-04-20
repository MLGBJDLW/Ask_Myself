const SQLITE_UTC_RE = /^\d{4}-\d{2}-\d{2} \d{2}:\d{2}:\d{2}(?:\.\d+)?$/;

/**
 * Parse timestamps emitted by both browser-side ISO strings and SQLite
 * `datetime('now')` strings (which are UTC but omit timezone info).
 */
export function parseAppDate(input: string): Date {
  if (!input) {
    return new Date(Number.NaN);
  }

  const value = input.trim();
  if (!value) {
    return new Date(Number.NaN);
  }

  if (SQLITE_UTC_RE.test(value)) {
    return new Date(value.replace(' ', 'T') + 'Z');
  }

  const direct = new Date(value);
  if (!Number.isNaN(direct.getTime())) {
    return direct;
  }

  if (value.includes(' ')) {
    const fallback = new Date(value.replace(' ', 'T'));
    if (!Number.isNaN(fallback.getTime())) {
      return fallback;
    }
  }

  return new Date(Number.NaN);
}

export function appTimeMs(input: string): number {
  const parsed = parseAppDate(input);
  return parsed.getTime();
}

