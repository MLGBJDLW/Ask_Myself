import { useMemo } from 'react';

import type { UsageTimeSeriesPoint } from '../../lib/api';

type UsageHeatmapMode = 'tokens' | 'requests';

interface UsageContributionHeatmapProps {
  points: UsageTimeSeriesPoint[];
  mode: UsageHeatmapMode;
  locale: string;
  valueLabel: string;
  startAt?: string | null;
  endAt?: string | null;
}

interface HeatmapDay {
  key: string;
  date: Date;
  inRange: boolean;
  value: number;
}

interface HeatmapWeek {
  key: string;
  days: HeatmapDay[];
}

const DAY_MS = 24 * 60 * 60 * 1000;
const MAX_VISIBLE_DAYS = 371;
const LEVEL_CLASSES = [
  'bg-surface-3/70',
  'bg-accent/20',
  'bg-accent/40',
  'bg-accent/65',
  'bg-accent',
] as const;

export function UsageContributionHeatmap({
  points,
  mode,
  locale,
  valueLabel,
  startAt,
  endAt,
}: UsageContributionHeatmapProps) {
  const projection = useMemo(
    () => buildHeatmapProjection(points, mode, startAt, endAt),
    [endAt, mode, points, startAt],
  );
  const monthFormatter = useMemo(
    () => new Intl.DateTimeFormat(locale, { month: 'short', timeZone: 'UTC' }),
    [locale],
  );
  const dateFormatter = useMemo(
    () => new Intl.DateTimeFormat(locale, {
      year: 'numeric',
      month: 'short',
      day: 'numeric',
      timeZone: 'UTC',
    }),
    [locale],
  );
  const numberFormatter = useMemo(
    () => new Intl.NumberFormat(locale, {
      maximumFractionDigits: 0,
      notation: 'compact',
    }),
    [locale],
  );

  if (projection.weeks.length === 0) return null;

  return (
    <div
      className="mt-4 overflow-x-auto pb-1"
      data-testid="usage-contribution-heatmap"
      role="group"
      aria-label={valueLabel}
    >
      <div className="min-w-max">
        <div
          className="mb-1 grid gap-1 text-[10px] text-text-tertiary"
          style={{ gridTemplateColumns: `repeat(${projection.weeks.length}, 0.75rem)` }}
          aria-hidden="true"
        >
          {projection.weeks.map((week, index) => {
            const firstInRangeDay = week.days.find((day) => day.inRange);
            const currentMonth = firstInRangeDay?.date.getUTCMonth();
            const previousMonth = index > 0
              ? projection.weeks[index - 1].days.find((day) => day.inRange)?.date.getUTCMonth()
              : undefined;
            const showMonth = firstInRangeDay != null && currentMonth !== previousMonth;
            return (
              <span key={`month-${week.key}`} className="h-3 whitespace-nowrap">
                {showMonth ? monthFormatter.format(firstInRangeDay.date) : ''}
              </span>
            );
          })}
        </div>

        <div
          className="grid grid-flow-col grid-rows-7 gap-1"
          role="grid"
          aria-rowcount={7}
          aria-colcount={projection.weeks.length}
        >
          {projection.weeks.flatMap((week) => week.days.map((day) => {
            const level = heatLevel(day.value, projection.maxValue);
            const formattedValue = numberFormatter.format(day.value);
            const label = `${dateFormatter.format(day.date)} · ${formattedValue} ${valueLabel}`;
            return (
              <span
                key={day.key}
                role="gridcell"
                aria-label={day.inRange ? label : undefined}
                title={day.inRange ? label : undefined}
                className={`h-3 w-3 rounded-[3px] ring-1 ring-inset ring-border/25 transition-transform hover:scale-125 focus-visible:scale-125 focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-accent ${
                  day.inRange ? LEVEL_CLASSES[level] : 'pointer-events-none opacity-0'
                }`}
                tabIndex={day.inRange ? 0 : -1}
              />
            );
          }))}
        </div>

        <div className="mt-2 flex items-center justify-end gap-1" aria-hidden="true">
          {LEVEL_CLASSES.map((levelClass, index) => (
            <span
              key={`${levelClass}-${index}`}
              className={`h-2.5 w-2.5 rounded-[2px] ring-1 ring-inset ring-border/25 ${levelClass}`}
            />
          ))}
        </div>
      </div>
    </div>
  );
}

function buildHeatmapProjection(
  points: UsageTimeSeriesPoint[],
  mode: UsageHeatmapMode,
  startAt?: string | null,
  endAt?: string | null,
): { weeks: HeatmapWeek[]; maxValue: number } {
  const valueByDate = new Map<string, number>();
  for (const point of points) {
    const key = point.date.slice(0, 10);
    const value = mode === 'tokens'
      ? point.promptTokens + point.completionTokens + point.thinkingTokens
      : point.requestCount;
    valueByDate.set(key, (valueByDate.get(key) ?? 0) + value);
  }

  const today = utcDay(new Date());
  let rangeEnd = endAt ? utcDay(new Date(endAt)) : today;
  if (!Number.isFinite(rangeEnd.getTime())) rangeEnd = today;
  if (rangeEnd.getTime() > today.getTime()) rangeEnd = today;

  let rangeStart = startAt ? utcDay(new Date(startAt)) : addUtcDays(rangeEnd, -(MAX_VISIBLE_DAYS - 1));
  if (!Number.isFinite(rangeStart.getTime())) {
    rangeStart = addUtcDays(rangeEnd, -(MAX_VISIBLE_DAYS - 1));
  }
  if (rangeStart.getTime() > rangeEnd.getTime()) rangeStart = rangeEnd;
  if (daysBetween(rangeStart, rangeEnd) + 1 > MAX_VISIBLE_DAYS) {
    rangeStart = addUtcDays(rangeEnd, -(MAX_VISIBLE_DAYS - 1));
  }

  const gridStart = addUtcDays(rangeStart, -rangeStart.getUTCDay());
  const gridEnd = addUtcDays(rangeEnd, 6 - rangeEnd.getUTCDay());
  const weeks: HeatmapWeek[] = [];
  let maxValue = 0;

  for (let cursor = gridStart; cursor.getTime() <= gridEnd.getTime(); cursor = addUtcDays(cursor, 7)) {
    const days: HeatmapDay[] = [];
    for (let offset = 0; offset < 7; offset += 1) {
      const date = addUtcDays(cursor, offset);
      const key = dateKey(date);
      const inRange = date.getTime() >= rangeStart.getTime() && date.getTime() <= rangeEnd.getTime();
      const value = inRange ? (valueByDate.get(key) ?? 0) : 0;
      maxValue = Math.max(maxValue, value);
      days.push({ key, date, inRange, value });
    }
    weeks.push({ key: dateKey(cursor), days });
  }

  return { weeks, maxValue };
}

function heatLevel(value: number, maxValue: number): number {
  if (value <= 0 || maxValue <= 0) return 0;
  return Math.min(4, Math.max(1, Math.ceil((Math.log1p(value) / Math.log1p(maxValue)) * 4)));
}

function utcDay(date: Date): Date {
  return new Date(Date.UTC(date.getUTCFullYear(), date.getUTCMonth(), date.getUTCDate()));
}

function addUtcDays(date: Date, days: number): Date {
  return new Date(date.getTime() + days * DAY_MS);
}

function daysBetween(start: Date, end: Date): number {
  return Math.floor((end.getTime() - start.getTime()) / DAY_MS);
}

function dateKey(date: Date): string {
  return date.toISOString().slice(0, 10);
}
