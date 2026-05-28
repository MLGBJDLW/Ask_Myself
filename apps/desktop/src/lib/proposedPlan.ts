import type { ArtifactPayload } from "../types/conversation";

export interface ProposedPlanArtifact {
  kind: "proposedPlan";
  version?: number;
  mode?: "plan" | string;
  title: string;
  markdown: string;
}

const PROPOSED_PLAN_RE = /<proposed_plan>\s*([\s\S]*?)\s*<\/proposed_plan>/i;
const PROPOSED_PLAN_GLOBAL_RE = /<proposed_plan>\s*[\s\S]*?\s*<\/proposed_plan>/gi;

function asRecord(value: unknown): Record<string, unknown> | null {
  return value && typeof value === "object" && !Array.isArray(value)
    ? (value as Record<string, unknown>)
    : null;
}

function normalizePlan(value: unknown): ProposedPlanArtifact | null {
  const record = asRecord(value);
  if (!record || record.kind !== "proposedPlan") return null;
  const markdown = typeof record.markdown === "string" ? record.markdown.trim() : "";
  if (!markdown) return null;
  const title = typeof record.title === "string" && record.title.trim()
    ? record.title.trim()
    : proposedPlanTitle(markdown);
  return {
    kind: "proposedPlan",
    version: typeof record.version === "number" ? record.version : undefined,
    mode: typeof record.mode === "string" ? record.mode : "plan",
    title,
    markdown,
  };
}

export function extractProposedPlanFromArtifacts(
  artifacts: ArtifactPayload | null | undefined,
): ProposedPlanArtifact | null {
  if (!artifacts) return null;
  if (Array.isArray(artifacts)) {
    for (const item of artifacts) {
      const found = extractProposedPlanFromArtifacts(item as ArtifactPayload);
      if (found) return found;
    }
    return null;
  }

  const direct = normalizePlan(artifacts);
  if (direct) return direct;

  const record = asRecord(artifacts);
  if (!record) return null;
  for (const key of ["proposedPlan", "plan", "artifacts", "trace"]) {
    const found = extractProposedPlanFromArtifacts(record[key] as ArtifactPayload);
    if (found) return found;
  }
  return null;
}

export function extractProposedPlanFromContent(content: string): ProposedPlanArtifact | null {
  const match = PROPOSED_PLAN_RE.exec(content);
  const markdown = match?.[1]?.trim() ?? "";
  if (!markdown) return null;
  return {
    kind: "proposedPlan",
    mode: "plan",
    title: proposedPlanTitle(markdown),
    markdown,
  };
}

export function extractProposedPlan(
  artifacts: ArtifactPayload | null | undefined,
  content: string,
): ProposedPlanArtifact | null {
  return extractProposedPlanFromArtifacts(artifacts) ?? extractProposedPlanFromContent(content);
}

export function stripProposedPlanBlock(content: string): string {
  return content.replace(PROPOSED_PLAN_GLOBAL_RE, "").trim();
}

function proposedPlanTitle(markdown: string): string {
  for (const line of markdown.split(/\r?\n/)) {
    const trimmed = line.trim();
    if (!trimmed) continue;
    const withoutHeading = trimmed.replace(/^#+\s*/, "");
    const withoutNumber = withoutHeading.replace(/^\d+[\.)]\s*/, "");
    const title = withoutNumber.replace(/^[*`]+|[*`:]+$/g, "").trim();
    if (title) return Array.from(title).slice(0, 96).join("");
  }
  return "Proposed plan";
}
