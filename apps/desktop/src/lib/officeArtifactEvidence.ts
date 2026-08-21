import type { ArtifactPayload } from '../types/conversation';

export type RecordValue = Record<string, unknown>;

export function officeRecord(value: unknown): RecordValue | null {
  return value && typeof value === 'object' && !Array.isArray(value)
    ? value as RecordValue
    : null;
}

export function officeText(value: unknown): string | null {
  return typeof value === 'string' && value.trim() ? value : null;
}

export interface OfficeArtifactEvidence {
  kind: string;
  payload: RecordValue;
}

export interface OfficeArtifactProofSummary {
  artifactSha256: string | null;
  renderArtifactSha256: string | null;
  renderShaBound: boolean | null;
  validationStatus: string | null;
  preservationStatus: 'pass' | 'failed' | null;
  schemaStatus: string | null;
  calculationStatus: string | null;
  calculationEngine: string | null;
  nativeEngine: string | null;
  nativeEngineVersion: string | null;
  nativeOpenSave: boolean | null;
  renderStatus: 'complete' | 'incomplete' | null;
  renderedSurfaces: number | null;
  expectedSurfaces: number | null;
  warningCount: number;
}

export function extractOfficeArtifactEvidence(
  artifacts: ArtifactPayload | undefined,
): OfficeArtifactEvidence | null {
  const payload = officeRecord(artifacts);
  const kind = officeText(payload?.kind);
  if (!payload || !kind || (!kind.startsWith('officeArtifact') && !kind.startsWith('officeLive'))) return null;
  return { kind, payload };
}

function finiteNumber(value: unknown): number | null {
  return typeof value === 'number' && Number.isFinite(value) ? value : null;
}

export function summarizeOfficeArtifactEvidence(
  evidence: OfficeArtifactEvidence,
): OfficeArtifactProofSummary {
  const payload = evidence.payload;
  const validation = officeRecord(payload.validation);
  const preservation = officeRecord(payload.preservationEvidence);
  const schema = officeRecord(payload.schemaValidation);
  const calculation = officeRecord(payload.calculationEvidence);
  const native = officeRecord(payload.nativeEvidence);
  const render = officeRecord(payload.renderEvidence);
  const artifactSha256 = officeText(payload.sha256);
  const renderArtifactSha256 = officeText(render?.artifactSha256);
  const validationBackend = officeRecord(validation?.backend);
  const validationContract = officeRecord(validationBackend?.contract) ?? officeRecord(validation?.contract);
  const warnings = Array.isArray(payload.warnings) ? payload.warnings : [];
  return {
    artifactSha256,
    renderArtifactSha256,
    renderShaBound: artifactSha256 && renderArtifactSha256
      ? artifactSha256 === renderArtifactSha256
      : null,
    validationStatus: officeText(validation?.status) ?? officeText(validationContract?.status),
    preservationStatus: preservation
      ? preservation.verified === true ? 'pass' : 'failed'
      : null,
    schemaStatus: officeText(schema?.status),
    calculationStatus: officeText(calculation?.level) ?? officeText(calculation?.profile),
    calculationEngine: officeText(calculation?.engine),
    nativeEngine: officeText(native?.engine),
    nativeEngineVersion: officeText(native?.engineVersion),
    nativeOpenSave: native ? native.nativeOpenSave === true : null,
    renderStatus: render ? render.complete === true ? 'complete' : 'incomplete' : null,
    renderedSurfaces: finiteNumber(render?.renderedSurfaces),
    expectedSurfaces: finiteNumber(render?.expectedSurfaces),
    warningCount: warnings.length,
  };
}
