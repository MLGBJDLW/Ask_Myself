import type { TranslationKey } from '../i18n';

type TranslationFn = (key: TranslationKey, params?: Record<string, string | number>) => string;

/**
 * Compact relative time for sidebar use: "just now", "2m", "1h", "3d", "2mo"
 */
export function relativeTime(iso: string, t: TranslationFn): string {
  const diff = Date.now() - new Date(iso).getTime();
  const secs = Math.floor(diff / 1000);
  if (secs < 60) return t('time.justNow');
  const mins = Math.floor(secs / 60);
  if (mins < 60) return `${mins}${t('time.minuteShort')}`;
  const hrs = Math.floor(mins / 60);
  if (hrs < 24) return `${hrs}${t('time.hourShort')}`;
  const days = Math.floor(hrs / 24);
  if (days < 30) return `${days}${t('time.dayShort')}`;
  const months = Math.floor(days / 30);
  return `${months}${t('time.monthShort')}`;
}

/**
 * Extended relative time for message timestamps:
 * "just now", "2m ago", "1h ago", "yesterday", "Feb 20"
 */
export function messageTimestamp(iso: string, t: TranslationFn): string {
  const date = new Date(iso);
  const now = new Date();
  const diff = now.getTime() - date.getTime();
  const secs = Math.floor(diff / 1000);

  if (secs < 60) return t('time.justNow');

  const mins = Math.floor(secs / 60);
  if (mins < 60) return t('time.minutesAgo', { n: mins });

  const hrs = Math.floor(mins / 60);
  if (hrs < 24) return t('time.hoursAgo', { n: hrs });

  // Check if yesterday
  const startOfToday = new Date(now.getFullYear(), now.getMonth(), now.getDate());
  const startOfYesterday = new Date(startOfToday.getTime() - 86_400_000);
  if (date >= startOfYesterday && date < startOfToday) {
    return t('chat.yesterday');
  }

  // For older dates, use locale-aware short date
  return date.toLocaleDateString(undefined, { month: 'short', day: 'numeric' });
}

/**
 * Returns true if two timestamps are more than `thresholdMs` apart.
 * Default threshold is 5 minutes.
 */
export function hasTimeGap(
  isoA: string | null | undefined,
  isoB: string,
  thresholdMs = 5 * 60 * 1000,
): boolean {
  if (!isoA) return false;
  return Math.abs(new Date(isoB).getTime() - new Date(isoA).getTime()) > thresholdMs;
}
