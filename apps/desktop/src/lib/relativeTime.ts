import type { TranslationKey } from '../i18n';
import { appTimeMs, parseAppDate } from './dateTime';

type TranslationFn = (key: TranslationKey, params?: Record<string, string | number>) => string;

/**
 * Compact relative time for sidebar use: "just now", "2m", "1h", "3d", "2mo"
 */
export function relativeTime(iso: string, t: TranslationFn): string {
  const ts = appTimeMs(iso);
  if (Number.isNaN(ts)) return t('time.justNow');
  const diff = Math.max(0, Date.now() - ts);
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
  const date = parseAppDate(iso);
  if (Number.isNaN(date.getTime())) return t('time.justNow');
  const now = new Date();
  const diff = Math.max(0, now.getTime() - date.getTime());
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
  const a = appTimeMs(isoA);
  const b = appTimeMs(isoB);
  if (Number.isNaN(a) || Number.isNaN(b)) return false;
  return Math.abs(b - a) > thresholdMs;
}
