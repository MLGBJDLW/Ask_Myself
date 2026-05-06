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

export interface QualityEvalReport {
  status: string;
  total: number;
  passed: number;
  failed: number;
  suites: QualityEvalSuiteReport[];
  behavioralEval: BehavioralEvalReport;
}
