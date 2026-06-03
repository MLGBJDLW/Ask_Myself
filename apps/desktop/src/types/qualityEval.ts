import type { StoredTrajectoryEvalReport } from './trace';

export interface BehavioralEvalCaseResult {
  id: string;
  passed: boolean;
  route: string;
  expectedRoute: string;
  evidenceMode: string;
  expectedEvidenceMode: string | null;
  missingTools: string[];
  missingPlanTools: string[];
  forbiddenToolsPresent: string[];
}

export interface BehavioralEvalReport {
  status: string;
  total: number;
  passed: number;
  failed: number;
  cases: BehavioralEvalCaseResult[];
}

export interface QualityEvalCheckResult {
  id: string;
  passed: boolean;
  detail: string;
}

export interface QualityEvalCaseResult {
  id: string;
  label: string;
  severity: string;
  passed: boolean;
  checks: QualityEvalCheckResult[];
}

export interface QualityEvalSuiteReport {
  id: string;
  label: string;
  total: number;
  passed: number;
  failed: number;
  cases: QualityEvalCaseResult[];
}

export interface QualityGateThresholds {
  maxFailed: number;
  minPassRate: number;
  requiredSuites: string[];
}

export interface QualityGateSuiteStatus {
  id: string;
  present: boolean;
  passed: boolean;
  failed: number;
}

export interface QualityGateReport {
  passed: boolean;
  passRate: number;
  thresholds: QualityGateThresholds;
  missingRequiredSuites: string[];
  failingRequiredSuites: string[];
  suites: QualityGateSuiteStatus[];
}

export interface QualityEvalReport {
  status: string;
  total: number;
  passed: number;
  failed: number;
  suites: QualityEvalSuiteReport[];
  behavioralEval: BehavioralEvalReport;
  gate: QualityGateReport;
}

export interface DeveloperEvalSmokeReport {
  status: string;
  total: number;
  passed: number;
  failed: number;
  qualityEval: QualityEvalReport;
  storedTrajectoryEval: StoredTrajectoryEvalReport;
}
