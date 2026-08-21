import {
  AlertTriangle,
  CheckCircle2,
  FileArchive,
  RotateCcw,
  ShieldCheck,
} from 'lucide-react';

import {
  extractOfficeArtifactEvidence,
  officeRecord as record,
  officeText as text,
  summarizeOfficeArtifactEvidence,
  type OfficeArtifactEvidence,
  type RecordValue,
} from '../../lib/officeArtifactEvidence';

export { extractOfficeArtifactEvidence };
export type { OfficeArtifactEvidence };

function statusTone(status: string): string {
  return ['ready', 'candidate', 'published', 'restored', 'pass'].includes(status)
    ? 'border-success/25 bg-success/10 text-success'
    : ['blocked', 'failed', 'error'].includes(status)
      ? 'border-danger/25 bg-danger/10 text-danger'
      : 'border-border/60 bg-surface-0/70 text-text-secondary';
}

function EvidencePill({ label, status }: { label: string; status: string }) {
  const passing = ['pass', 'ready', 'complete', 'native', 'compatible', 'static'].includes(status);
  return (
    <span className={`inline-flex items-center gap-1 rounded-md border px-2 py-1 text-[10px] ${
      passing
        ? 'border-success/20 bg-success/8 text-success'
        : 'border-border/55 bg-surface-0/60 text-text-secondary'
    }`}>
      {passing && <CheckCircle2 className="h-3 w-3" />}
      {label}: {status}
    </span>
  );
}

export function OfficeArtifactEvidencePanel({ evidence }: { evidence: OfficeArtifactEvidence }) {
  const payload = evidence.payload;
  const proof = summarizeOfficeArtifactEvidence(evidence);
  const status = text(payload.status) ?? (payload.ready === true ? 'ready' : 'information');
  const assessment = record(payload.assessment) ?? (evidence.kind === 'officeArtifactAssessment' ? payload : null);
  const guarantees = record(assessment?.guarantees);
  const plan = record(assessment?.adapterPlan);
  const steps = Array.isArray(plan?.steps)
    ? plan.steps.map(record).filter((value): value is RecordValue => value !== null)
    : [];
  const blockers = Array.isArray(payload.blockers)
    ? payload.blockers.map(record).filter((value): value is RecordValue => value !== null)
    : [];
  const preservation = record(payload.preservationEvidence);
  const calculation = record(payload.calculationEvidence);
  const render = record(payload.renderEvidence);
  const schema = record(payload.schemaValidation);
  const synthetic = record(payload.syntheticPreview);
  const validation = record(payload.validation);
  const candidateId = text(payload.candidateId);
  const receiptId = text(payload.receiptId);
  const destination = text(payload.destination) ?? text(payload.path);
  const errorMessage = evidence.kind === 'officeArtifactError' ? text(payload.message) : null;
  const endpoint = text(payload.endpoint);
  const pairingCode = text(payload.pairingCode);
  const addInManifestPath = text(payload.addInManifestPath);
  const sessions = Array.isArray(payload.sessions)
    ? payload.sessions.map(record).filter((value): value is RecordValue => value !== null)
    : [];
  const limitations = record(assessment?.limitations);
  const rawConsent = assessment?.consentRequired;
  const consent = Array.isArray(rawConsent)
    ? rawConsent.filter((value): value is string => typeof value === 'string')
    : [];

  return (
    <div
      data-testid="office-artifact-evidence"
      className="rounded-lg border border-border/65 bg-surface-1/80 p-3 text-xs"
    >
      <div className="flex flex-wrap items-center gap-2">
        <span className="inline-flex items-center gap-1.5 font-medium text-text-primary">
          <FileArchive className="h-3.5 w-3.5 text-accent" />
          Office artifact
        </span>
        {text(payload.format) && (
          <span className="uppercase tracking-[0.12em] text-text-tertiary">{text(payload.format)}</span>
        )}
        <span className={`rounded-md border px-2 py-0.5 text-[10px] ${statusTone(status)}`}>
          {status}
        </span>
      </div>

      {(candidateId || receiptId || destination) && (
        <div className="mt-2 grid gap-1 text-[11px] text-text-secondary sm:grid-cols-2">
          {candidateId && <div className="truncate">Candidate · {candidateId}</div>}
          {receiptId && (
            <div className="inline-flex items-center gap-1 truncate">
              <RotateCcw className="h-3 w-3" /> Receipt · {receiptId}
            </div>
          )}
          {destination && <div className="truncate sm:col-span-2">{destination}</div>}
        </div>
      )}

      {(endpoint || pairingCode || addInManifestPath || sessions.length > 0) && (
        <div className="mt-2 space-y-1 rounded-md border border-border/50 bg-surface-0/45 px-2.5 py-2 text-[11px] text-text-secondary">
          {endpoint && <div>Loopback · {endpoint}</div>}
          {pairingCode && <div className="font-mono text-sm tracking-[0.2em] text-text-primary">{pairingCode}</div>}
          {addInManifestPath && <div className="truncate">Add-in manifest · {addInManifestPath}</div>}
          {sessions.map((session) => (
            <div key={text(session.sessionId) ?? JSON.stringify(session)} className="truncate">
              {text(session.host) ?? 'Office'} · {text(session.documentId) ?? 'active document'} · {text(session.sessionId)}
            </div>
          ))}
        </div>
      )}

      {guarantees && (
        <div className="mt-2 flex flex-wrap gap-1.5">
          {['preservation', 'calculation', 'render'].map((key) => {
            const value = text(guarantees[key]);
            return value ? <EvidencePill key={key} label={key} status={value} /> : null;
          })}
        </div>
      )}

      {steps.length > 0 && (
        <div className="mt-2 rounded-md border border-border/50 bg-surface-0/45 px-2.5 py-2">
          <div className="mb-1 text-[10px] uppercase tracking-[0.14em] text-text-tertiary">Adapter plan</div>
          <div className="flex flex-wrap gap-1.5">
            {steps.map((step, index) => (
              <span key={`${text(step.step) ?? 'step'}-${index}`} className="rounded border border-border/50 px-1.5 py-0.5 text-[10px] text-text-secondary">
                {text(step.step) ?? 'step'} → {text(step.adapter) ?? 'unknown'}
              </span>
            ))}
          </div>
        </div>
      )}

      <div className="mt-2 flex flex-wrap gap-1.5">
        {validation && <EvidencePill label="contract" status={proof.validationStatus ?? 'recorded'} />}
        {preservation && <EvidencePill label="preservation" status={proof.preservationStatus ?? 'recorded'} />}
        {schema && <EvidencePill label="Open XML SDK" status={proof.schemaStatus ?? 'recorded'} />}
        {calculation && <EvidencePill label="calculation" status={proof.calculationStatus ?? 'recorded'} />}
        {render && <EvidencePill label="render" status={proof.renderStatus ?? 'incomplete'} />}
        {proof.nativeEngine && (
          <EvidencePill label="native host" status={proof.nativeOpenSave ? 'pass' : 'failed'} />
        )}
        {proof.renderShaBound !== null && (
          <EvidencePill label="render SHA" status={proof.renderShaBound ? 'pass' : 'mismatch'} />
        )}
        {synthetic && <EvidencePill label="synthetic preview" status="not final render" />}
      </div>

      {(proof.artifactSha256 || proof.nativeEngine || proof.renderedSurfaces !== null || proof.warningCount > 0) && (
        <div className="mt-2 grid gap-1 rounded-md border border-border/50 bg-surface-0/45 px-2.5 py-2 text-[10px] text-text-secondary sm:grid-cols-2">
          {proof.artifactSha256 && (
            <div className="truncate font-mono" title={proof.artifactSha256}>
              SHA-256 · {proof.artifactSha256}
            </div>
          )}
          {proof.nativeEngine && (
            <div className="truncate">
              Native · {proof.nativeEngine}{proof.nativeEngineVersion ? ` ${proof.nativeEngineVersion}` : ''}
            </div>
          )}
          {proof.renderedSurfaces !== null && (
            <div>
              Rendered surfaces · {proof.renderedSurfaces}
              {proof.expectedSurfaces !== null ? ` / ${proof.expectedSurfaces}` : ''}
            </div>
          )}
          {proof.calculationEngine && <div className="truncate">Calculation · {proof.calculationEngine}</div>}
          {proof.warningCount > 0 && <div className="text-warning">Warnings · {proof.warningCount}</div>}
        </div>
      )}

      {consent.length > 0 && (
        <div className="mt-2 flex items-start gap-1.5 rounded-md border border-warning/25 bg-warning/8 px-2.5 py-2 text-[11px] text-warning">
          <ShieldCheck className="mt-0.5 h-3 w-3 shrink-0" />
          Explicit consent required: {consent.join(', ')}
        </div>
      )}
      {errorMessage && (
        <div className="mt-2 flex items-start gap-1.5 rounded-md border border-danger/25 bg-danger/8 px-2.5 py-2 text-[11px] text-danger">
          <AlertTriangle className="mt-0.5 h-3 w-3 shrink-0" />
          {errorMessage}
        </div>
      )}
      {blockers.length > 0 && (
        <div className="mt-2 space-y-1 rounded-md border border-danger/25 bg-danger/8 px-2.5 py-2 text-[11px] text-danger">
          {blockers.slice(0, 5).map((blocker, index) => (
            <div key={`${text(blocker.code) ?? 'blocker'}-${index}`} className="flex items-start gap-1.5">
              <AlertTriangle className="mt-0.5 h-3 w-3 shrink-0" />
              <span>{text(blocker.message) ?? text(blocker.detail) ?? text(blocker.code) ?? 'Blocked'}</span>
            </div>
          ))}
        </div>
      )}
      {limitations && Object.keys(limitations).length > 0 && (
        <details className="mt-2 text-[10px] text-text-tertiary">
          <summary className="cursor-pointer">Adapter limitations</summary>
          <ul className="mt-1 list-disc space-y-0.5 pl-4">
            {Object.entries(limitations).flatMap(([adapter, values]) => (
              Array.isArray(values)
                ? values.filter((value): value is string => typeof value === 'string').map((value) => (
                    <li key={`${adapter}-${value}`}>{adapter}: {value}</li>
                  ))
                : []
            ))}
          </ul>
        </details>
      )}
    </div>
  );
}
