import { useCallback, useMemo, useState } from 'react';
import type { ApprovalDecisionValue, ApprovalRequest } from '../../types';
import { approveToolCall } from '../../lib/api';

interface ApprovalDialogProps {
  request: ApprovalRequest | null;
  /** Called after the user clicks a button and the backend confirms. */
  onResolved?: (request: ApprovalRequest, decision: ApprovalDecisionValue) => void;
}

const RISK_COLOR: Record<ApprovalRequest['riskLevel'], string> = {
  low: 'bg-emerald-50 text-emerald-700 border-emerald-200',
  medium: 'bg-amber-50 text-amber-700 border-amber-200',
  high: 'bg-rose-50 text-rose-700 border-rose-200',
};

const RISK_LABEL: Record<ApprovalRequest['riskLevel'], string> = {
  low: 'LOW RISK',
  medium: 'MEDIUM RISK',
  high: 'HIGH RISK',
};

export function ApprovalDialog({ request, onResolved }: ApprovalDialogProps) {
  const [busy, setBusy] = useState(false);
  const [showAdvanced, setShowAdvanced] = useState(false);

  const preview = useMemo(() => request?.argumentsPreview ?? '', [request]);

  const decide = useCallback(
    async (decision: ApprovalDecisionValue) => {
      if (!request || busy) return;
      setBusy(true);
      try {
        await approveToolCall(request.id, decision);
        onResolved?.(request, decision);
      } catch (err) {
        console.error('[approval] decision failed', err);
      } finally {
        setBusy(false);
      }
    },
    [request, busy, onResolved],
  );

  if (!request) return null;

  return (
    <div
      role="dialog"
      aria-modal="true"
      aria-labelledby="approval-dialog-title"
      className="fixed inset-0 z-50 flex items-center justify-center bg-black/40 backdrop-blur-sm"
    >
      <div className="w-full max-w-lg rounded-xl bg-white shadow-xl dark:bg-zinc-900 dark:text-zinc-100">
        <div className="flex items-center gap-3 border-b border-zinc-200 px-5 py-4 dark:border-zinc-700">
          <span aria-hidden className="text-2xl">🛠</span>
          <h2 id="approval-dialog-title" className="flex-1 text-lg font-semibold">
            {request.toolName}
          </h2>
          <span
            className={`rounded-full border px-2 py-0.5 text-xs font-semibold ${RISK_COLOR[request.riskLevel]}`}
          >
            {RISK_LABEL[request.riskLevel]}
          </span>
        </div>

        <div className="space-y-3 px-5 py-4">
          <p className="text-sm text-zinc-700 dark:text-zinc-300">{request.reason}</p>
          {request.checkpointPreview?.planned && (
            <div className="rounded-md border border-emerald-200 bg-emerald-50 px-3 py-2 text-xs text-emerald-800 dark:border-emerald-800 dark:bg-emerald-950/40 dark:text-emerald-200">
              <div className="font-semibold">Safety checkpoint will be saved first</div>
              <div className="mt-1">{request.checkpointPreview.note}</div>
              {request.checkpointPreview.targetPaths.length > 0 && (
                <ul className="mt-1 list-inside list-disc space-y-0.5">
                  {request.checkpointPreview.targetPaths.map(path => (
                    <li key={path} className="break-all font-mono">{path}</li>
                  ))}
                </ul>
              )}
            </div>
          )}
          <details className="rounded-md border border-zinc-200 dark:border-zinc-700">
            <summary className="cursor-pointer select-none px-3 py-2 text-xs font-medium text-zinc-600 dark:text-zinc-400">
              Arguments
            </summary>
            <pre className="max-h-64 overflow-auto px-3 py-2 text-xs">
              <code>{preview}</code>
            </pre>
          </details>
        </div>

        <div className="flex flex-wrap items-center justify-end gap-2 border-t border-zinc-200 px-5 py-3 dark:border-zinc-700">
          <button
            type="button"
            disabled={busy}
            onClick={() => decide('deny')}
            className="rounded-md border border-zinc-300 px-3 py-1.5 text-sm font-medium hover:bg-zinc-100 disabled:opacity-60 dark:border-zinc-600 dark:hover:bg-zinc-800"
          >
            Deny
          </button>
          <button
            type="button"
            disabled={busy}
            onClick={() => decide('allow_session')}
            className="rounded-md border border-zinc-300 px-3 py-1.5 text-sm font-medium hover:bg-zinc-100 disabled:opacity-60 dark:border-zinc-600 dark:hover:bg-zinc-800"
          >
            Allow (Session)
          </button>
          <button
            type="button"
            disabled={busy}
            onClick={() => decide('allow_once')}
            className="rounded-md bg-emerald-600 px-3 py-1.5 text-sm font-medium text-white hover:bg-emerald-700 disabled:opacity-60"
          >
            Allow Once
          </button>
        </div>

        <div className="border-t border-zinc-200 px-5 py-2 text-xs text-zinc-500 dark:border-zinc-700 dark:text-zinc-400">
          <button
            type="button"
            onClick={() => setShowAdvanced(v => !v)}
            className="underline-offset-2 hover:underline"
          >
            {showAdvanced ? 'Hide advanced' : 'Advanced'}
          </button>
          {showAdvanced && (
            <div className="mt-2">
              <button
                type="button"
                disabled={busy}
                onClick={() => decide('never')}
                className="rounded-md border border-rose-300 px-2 py-1 text-xs font-medium text-rose-700 hover:bg-rose-50 disabled:opacity-60 dark:border-rose-700 dark:text-rose-300 dark:hover:bg-rose-950"
              >
                Never allow this tool
              </button>
            </div>
          )}
        </div>
      </div>
    </div>
  );
}

export default ApprovalDialog;
