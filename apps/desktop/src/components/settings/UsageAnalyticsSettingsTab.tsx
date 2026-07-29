import { save } from '@tauri-apps/plugin-dialog';
import { CalendarDays, Download, RefreshCw, Trash2 } from 'lucide-react';
import { useMemo, useState } from 'react';
import { toast } from 'sonner';

import * as api from '../../lib/api';
import { useUsageAnalytics } from '../../features/usage/useUsageAnalytics';

type RangePreset = 'today' | '7d' | '30d' | '90d' | 'year' | 'all' | 'custom';

export function UsageAnalyticsSettingsTab() {
  const { filter, setFilter, data, loading, error, reload } = useUsageAnalytics(rangeFilter('30d'));
  const [range, setRange] = useState<RangePreset>('30d');
  const [customStart, setCustomStart] = useState('');
  const [customEnd, setCustomEnd] = useState('');
  const [chartMode, setChartMode] = useState<'tokens' | 'requests'>('tokens');
  const providers = useMemo(() => Array.from(new Set(data?.byModel.map((row) => row.providerId).filter(Boolean) as string[])), [data]);
  const models = useMemo(() => Array.from(new Set(data?.byModel.map((row) => row.modelId).filter(Boolean) as string[])), [data]);
  const operations = useMemo(() => data?.byOperation.map((row) => row.key) ?? [], [data]);

  const chooseRange = (preset: RangePreset) => {
    setRange(preset);
    if (preset !== 'custom') setFilter({
      ...rangeFilter(preset),
      providerId: filter.providerId ?? null,
      modelId: filter.modelId ?? null,
      operationKind: filter.operationKind ?? null,
      timeBucket: filter.timeBucket ?? 'day',
    });
  };
  const applyCustom = () => setFilter({
    ...filter,
    startAt: customStart ? new Date(`${customStart}T00:00:00`).toISOString() : null,
    endAt: customEnd ? new Date(`${customEnd}T23:59:59.999`).toISOString() : null,
  });
  const exportData = async (format: 'csv' | 'json') => {
    const path = await save({ defaultPath: `nexa-ai-usage.${format}`, filters: [{ name: format.toUpperCase(), extensions: [format] }] });
    if (!path) return;
    try { await api.exportAiUsage(filter, format, path); toast.success(`AI usage exported to ${path}`); }
    catch (reason) { toast.error(String(reason)); }
  };
  const remove = async (all: boolean) => {
    const targetFilter = all ? {} : filter;
    const label = all ? 'all AI usage statistics' : 'AI usage statistics in the selected range';
    if (!window.confirm(`Delete ${label}? This cannot be undone.`)) return;
    try {
      const count = await api.deleteAiUsageRecords(targetFilter);
      toast.success(`Deleted ${count} usage record${count === 1 ? '' : 's'}`);
      reload();
    } catch (reason) { toast.error(String(reason)); }
  };

  if (loading && !data) return <div className="rounded-xl border border-border bg-surface-1 p-8 text-sm text-text-tertiary">Loading canonical AI usage ledger…</div>;
  if (error && !data) return <div className="rounded-xl border border-danger/30 bg-danger/10 p-5 text-sm text-danger">{error}</div>;
  if (!data) return null;
  const totals = data.totals;
  const maxTrendValue = Math.max(1, ...data.timeSeries.map((point) => chartMode === 'tokens'
    ? point.promptTokens + point.completionTokens + point.thinkingTokens
    : point.requestCount));

  return (
    <div className="space-y-5" data-testid="usage-analytics-tab">
      <div className="flex flex-wrap items-start justify-between gap-3">
        <div>
          <h2 className="text-base font-semibold text-text-primary">AI Usage</h2>
          <p className="mt-1 max-w-2xl text-xs text-text-tertiary">Invocation-level accounting across main agents, subagents, and legacy runs. Language-model tokens are not mixed with voice, image, or audio units.</p>
        </div>
        <button type="button" onClick={reload} className="inline-flex items-center gap-1.5 rounded-md border border-border px-2.5 py-1.5 text-xs text-text-secondary hover:bg-surface-2"><RefreshCw size={13} /> Refresh</button>
      </div>

      <div className="flex flex-wrap items-center gap-2 rounded-lg border border-border bg-surface-1 p-2">
        <CalendarDays size={14} className="ml-1 text-text-tertiary" />
        {(['today', '7d', '30d', '90d', 'year', 'all', 'custom'] as RangePreset[]).map((preset) => (
          <button key={preset} type="button" onClick={() => chooseRange(preset)} className={`rounded-md px-2.5 py-1.5 text-xs ${range === preset ? 'bg-accent text-text-inverse' : 'text-text-secondary hover:bg-surface-2'}`}>{rangeLabel(preset)}</button>
        ))}
        <select value={filter.providerId ?? ''} onChange={(event) => setFilter({ ...filter, providerId: event.target.value || null })} className="ml-auto rounded-md border border-border bg-surface-0 px-2 py-1.5 text-xs text-text-primary"><option value="">All providers</option>{providers.map((provider) => <option key={provider}>{provider}</option>)}</select>
        <select value={filter.modelId ?? ''} onChange={(event) => setFilter({ ...filter, modelId: event.target.value || null })} className="rounded-md border border-border bg-surface-0 px-2 py-1.5 text-xs text-text-primary"><option value="">All models</option>{models.map((model) => <option key={model}>{model}</option>)}</select>
        <select value={filter.operationKind ?? ''} onChange={(event) => setFilter({ ...filter, operationKind: event.target.value || null })} className="rounded-md border border-border bg-surface-0 px-2 py-1.5 text-xs text-text-primary"><option value="">All operations</option>{operations.map((operation) => <option key={operation} value={operation}>{operationLabel(operation)}</option>)}</select>
      </div>
      {range === 'custom' && <div className="flex flex-wrap items-end gap-2 rounded-lg border border-border bg-surface-1 p-3"><label className="text-xs text-text-secondary">From<input type="date" value={customStart} onChange={(event) => setCustomStart(event.target.value)} className="ml-2 rounded-md border border-border bg-surface-0 px-2 py-1.5 text-text-primary" /></label><label className="text-xs text-text-secondary">To<input type="date" value={customEnd} onChange={(event) => setCustomEnd(event.target.value)} className="ml-2 rounded-md border border-border bg-surface-0 px-2 py-1.5 text-text-primary" /></label><button type="button" onClick={applyCustom} className="rounded-md bg-accent px-3 py-1.5 text-xs text-text-inverse">Apply</button></div>}

      <div className="grid gap-3 sm:grid-cols-2 lg:grid-cols-5">
        <Metric label="Total tokens" value={formatNumber(totals.totalTokens)} />
        <Metric label="Input tokens" value={formatNumber(totals.promptTokens)} />
        <Metric label="Output tokens" value={formatNumber(totals.completionTokens)} />
        <Metric label="Thinking tokens" value={formatNumber(totals.thinkingTokens)} />
        <Metric label="Requests" value={formatNumber(totals.requestCount)} />
        <Metric label="Agent runs" value={formatNumber(totals.agentRunCount)} />
        <Metric label="Cache read" value={formatNumber(totals.cacheReadTokens)} />
        <Metric label="Cache miss" value={formatNumber(totals.cacheMissTokens)} />
        <Metric label="Cache hit rate" value={totals.cacheHitRate == null ? '—' : `${totals.cacheHitRate.toFixed(1)}%`} />
        <Metric label="Estimated cost" value={formatCost(totals)} hint={totals.estimatedCostMicros == null ? 'Usage tracked · Cost unknown' : undefined} />
      </div>

      <section className="rounded-xl border border-border bg-surface-1 p-4">
        <div className="flex items-center justify-between gap-3"><h3 className="text-sm font-semibold text-text-primary">Coverage & accuracy</h3><span className="text-[11px] text-text-tertiary">Provider values take precedence over normalized and estimated values</span></div>
        <div className="mt-3 flex h-3 overflow-hidden rounded-full bg-surface-3" title="Usage coverage">
          <span className="bg-success" style={{ width: `${totals.providerReportedPercent}%` }} />
          <span className="bg-info" style={{ width: `${totals.normalizedPercent}%` }} />
          <span className="bg-warning" style={{ width: `${totals.estimatedPercent}%` }} />
          <span className="bg-danger" style={{ width: `${totals.unknownPercent}%` }} />
        </div>
        <div className="mt-2 flex flex-wrap gap-x-4 gap-y-1 text-[11px] text-text-tertiary"><span>Provider-reported {totals.providerReportedPercent.toFixed(1)}%</span><span>Normalized {totals.normalizedPercent.toFixed(1)}%</span><span>Estimated {totals.estimatedPercent.toFixed(1)}%</span><span>Unknown {totals.unknownPercent.toFixed(1)}%</span></div>
      </section>

      <section className="rounded-xl border border-border bg-surface-1 p-4">
        <div className="flex flex-wrap items-center justify-between gap-2"><h3 className="text-sm font-semibold text-text-primary">Usage trend</h3><div className="flex gap-2"><select value={filter.timeBucket ?? 'day'} onChange={(event) => setFilter({ ...filter, timeBucket: event.target.value as 'day' | 'week' | 'month' })} className="rounded-md border border-border bg-surface-0 px-2 py-1 text-[11px] text-text-primary"><option value="day">Daily</option><option value="week">Weekly</option><option value="month">Monthly</option></select><select value={chartMode} onChange={(event) => setChartMode(event.target.value as 'tokens' | 'requests')} className="rounded-md border border-border bg-surface-0 px-2 py-1 text-[11px] text-text-primary"><option value="tokens">Tokens</option><option value="requests">Requests</option></select></div></div>
        {data.timeSeries.length === 0 ? <Empty /> : <div className="mt-4 space-y-2">{data.timeSeries.map((point) => (
          <div key={point.date} className="grid grid-cols-[5.5rem_1fr_5rem] items-center gap-2 text-[11px]"><span className="text-text-tertiary">{point.date}</span><div className="flex h-3 overflow-hidden rounded-full bg-surface-3">{chartMode === 'tokens' ? <><span className="bg-accent" style={{ width: `${point.promptTokens / maxTrendValue * 100}%` }} /><span className="bg-info" style={{ width: `${point.completionTokens / maxTrendValue * 100}%` }} /><span className="bg-[var(--context-overhead)]" style={{ width: `${point.thinkingTokens / maxTrendValue * 100}%` }} /></> : <span className="bg-accent" style={{ width: `${point.requestCount / maxTrendValue * 100}%` }} />}</div><span className="text-right tabular-nums text-text-secondary">{formatNumber(chartMode === 'tokens' ? point.promptTokens + point.completionTokens + point.thinkingTokens : point.requestCount)}</span></div>
        ))}</div>}
      </section>

      <div className="grid gap-5 xl:grid-cols-2">
        <Breakdown title="Provider / model" rows={data.byModel} totals={totals} />
        <Breakdown title="Operation" rows={data.byOperation} totals={totals} />
      </div>

      <div className="flex flex-wrap justify-between gap-2 rounded-xl border border-border bg-surface-1 p-4">
        <div className="flex gap-2"><button type="button" onClick={() => void exportData('csv')} className="inline-flex items-center gap-1.5 rounded-md border border-border px-2.5 py-1.5 text-xs text-text-secondary"><Download size={13} /> Export CSV</button><button type="button" onClick={() => void exportData('json')} className="inline-flex items-center gap-1.5 rounded-md border border-border px-2.5 py-1.5 text-xs text-text-secondary"><Download size={13} /> Export JSON</button></div>
        <div className="flex gap-2"><button type="button" onClick={() => void remove(false)} className="inline-flex items-center gap-1.5 rounded-md border border-warning/40 px-2.5 py-1.5 text-xs text-warning"><Trash2 size={13} /> Delete selected range</button><button type="button" onClick={() => void remove(true)} className="inline-flex items-center gap-1.5 rounded-md border border-danger/40 px-2.5 py-1.5 text-xs text-danger"><Trash2 size={13} /> Reset all usage</button></div>
      </div>
    </div>
  );
}

function Metric({ label, value, hint }: { label: string; value: string; hint?: string }) { return <div className="rounded-xl border border-border bg-surface-1 p-3"><div className="text-[11px] text-text-tertiary">{label}</div><div className="mt-1 text-lg font-semibold tabular-nums text-text-primary">{value}</div>{hint && <div className="mt-1 text-[10px] text-text-tertiary">{hint}</div>}</div>; }
function Empty() { return <div className="mt-4 rounded-lg border border-dashed border-border p-6 text-center text-xs text-text-tertiary">No usage records in this range.</div>; }
function Breakdown({ title, rows, totals }: { title: string; rows: api.UsageBreakdownRow[]; totals: api.UsageTotals }) { return <section className="overflow-hidden rounded-xl border border-border bg-surface-1"><h3 className="border-b border-border px-4 py-3 text-sm font-semibold text-text-primary">{title}</h3>{rows.length === 0 ? <div className="p-4"><Empty /></div> : <div className="overflow-x-auto"><table className="w-full min-w-[720px] text-left text-[11px]"><thead className="text-text-tertiary"><tr><th className="px-3 py-2">Name</th><th>Token share</th><th>Request share</th><th>Prompt</th><th>Completion</th><th>Thinking</th><th>Cache hit</th><th>Avg/request</th><th>Avg/turn</th><th>Success</th><th className="pr-3">Cost</th></tr></thead><tbody>{rows.map((row) => { const cacheTotal = row.cacheReadTokens + row.cacheMissTokens; return <tr key={row.key} className="border-t border-border/60 text-text-secondary"><td className="max-w-48 truncate px-3 py-2 font-medium text-text-primary" title={row.key}>{operationLabel(row.key)}</td><td>{percent(row.totalTokens, totals.totalTokens)}</td><td>{percent(row.requestCount, totals.requestCount)}</td><td>{formatNumber(row.promptTokens)}</td><td>{formatNumber(row.completionTokens)}</td><td>{formatNumber(row.thinkingTokens)}</td><td>{cacheTotal ? `${row.cacheReadTokens / cacheTotal * 100 | 0}%` : '—'}</td><td>{formatNumber(row.requestCount ? row.totalTokens / row.requestCount : 0)}</td><td>{row.turnCount ? formatNumber(row.totalTokens / row.turnCount) : '—'}</td><td>{row.requestCount ? `${(row.successCount / row.requestCount * 100).toFixed(1)}%` : '—'}</td><td className="pr-3">{row.estimatedCostMicros == null ? 'Unknown' : `$${(row.estimatedCostMicros / 1_000_000).toFixed(4)}`}</td></tr>; })}</tbody></table></div>}</section>; }

function rangeFilter(preset: Exclude<RangePreset, 'custom'> | RangePreset): api.UsageAnalyticsFilter { const now = new Date(); if (preset === 'all' || preset === 'custom') return {}; const start = new Date(now); if (preset === 'today') start.setHours(0, 0, 0, 0); else if (preset === 'year') { start.setMonth(0, 1); start.setHours(0, 0, 0, 0); } else start.setDate(start.getDate() - Number.parseInt(preset, 10)); return { startAt: start.toISOString() }; }
function rangeLabel(preset: RangePreset): string { return ({ today: 'Today', '7d': '7 days', '30d': '30 days', '90d': '90 days', year: 'This year', all: 'All time', custom: 'Custom' })[preset]; }
function formatNumber(value: number): string { return new Intl.NumberFormat(undefined, { maximumFractionDigits: value < 100 ? 1 : 0, notation: value >= 1_000_000 ? 'compact' : 'standard' }).format(value); }
function percent(value: number, total: number): string { return total ? `${(value / total * 100).toFixed(1)}%` : '—'; }
function formatCost(totals: api.UsageTotals): string { return totals.estimatedCostMicros == null ? 'Unknown' : `${totals.currency ?? 'USD'} ${(totals.estimatedCostMicros / 1_000_000).toFixed(4)}`; }
function operationLabel(value: string): string { return ({ agent_main: 'Main Agent', subagent: 'Subagent', summarization: 'Summarization', compaction: 'Compaction', conversation_title: 'Title Generation', judge: 'Judge', dreaming: 'Dreaming', evolution: 'Evolution', quality_evaluation: 'Quality Evaluation', workflow: 'Workflow', other: 'Other', legacy_unclassified: 'Legacy / Unclassified' } as Record<string, string>)[value] ?? value; }
