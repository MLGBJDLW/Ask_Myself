import type { Skill } from "../types/extensions";
import { buildWorkflowBatchPrompt } from "./workflowPrompts";

export type SlashCommandKind = "command" | "skill" | "workflow";
export type SlashCommandAction = "prompt" | "compact" | "openWorkflows" | "planMode" | "companion";
export type SlashCommandExecutionMode = "normal" | "plan";

export interface SlashWorkflowTemplate {
  id: string;
  label: string;
  description: string;
  promptTemplate: string;
}

export interface SlashCommandOption {
  id: string;
  name: string;
  title: string;
  description: string;
  kind: SlashCommandKind;
  action: SlashCommandAction;
  sourceLabel: string;
  promptTemplate?: string;
  skillId?: string;
  skillName?: string;
  workflowTemplateId?: string;
  searchText: string;
}

export interface SlashCommandTrigger {
  start: number;
  end: number;
  query: string;
  token: string;
}

export interface ResolvedSlashCommand {
  command: SlashCommandOption;
  message: string;
  displayMessage?: string;
  skillIds: string[];
  localAction?: Exclude<SlashCommandAction, "prompt" | "planMode">;
  executionMode?: SlashCommandExecutionMode;
  artifact: Record<string, unknown>;
}

const COMMAND_NAME_PATTERN = "[a-zA-Z0-9_.:@-]+";
const FIRST_COMMAND_RE = new RegExp(`(^|\\s)/((${COMMAND_NAME_PATTERN}))(?:\\s|$)`);

const COMMON_COMMANDS: Array<Omit<SlashCommandOption, "id" | "kind" | "sourceLabel" | "searchText">> = [
  {
    name: "plan",
    title: "Plan",
    description: "Enter read-only Plan Mode and produce an approval-ready implementation plan.",
    action: "planMode",
  },

  {
    name: "goal",
    title: "Goal",
    description: "Set a persistent active goal with success criteria, constraints, and checkpoints.",
    action: "prompt",
    promptTemplate:
      "Start executing this durable conversation goal immediately. Establish success criteria and a short working plan, then keep taking concrete actions until the goal is actually achieved and verified. Do not stop after restating the goal, planning, or reporting partial progress. Ask only for information that genuinely blocks safe progress; otherwise continue autonomously. Use update_goal to mark the goal complete only after verification, or blocked only when external input is required.\n\nGoal:\n{{input}}",
  },
  {
    name: "review",
    title: "Review",
    description: "Review code or a proposal for defects, regressions, and missing tests.",
    action: "prompt",
    promptTemplate:
      "Review this as a senior engineer. Lead with concrete findings, severity, and file or behavior references.\n\nScope:\n{{input}}",
  },
  {
    name: "debug",
    title: "Debug",
    description: "Reproduce, isolate, instrument, fix, and regression-test a bug.",
    action: "prompt",
    promptTemplate:
      "Diagnose this bug rigorously: reproduce it, identify the smallest failing path, inspect likely causes, implement a focused fix, and verify it.\n\nBug:\n{{input}}",
  },
  {
    name: "refactor",
    title: "Refactor",
    description: "Improve structure while preserving behavior and keeping the change scoped.",
    action: "prompt",
    promptTemplate:
      "Refactor this area without changing behavior. First inspect existing patterns, then make the smallest structural improvement that pays for itself.\n\nTarget:\n{{input}}",
  },
  {
    name: "test",
    title: "Test",
    description: "Add or run focused tests for the behavior in question.",
    action: "prompt",
    promptTemplate:
      "Add or run the most relevant tests for this behavior. Prefer focused coverage over broad churn, and report exactly what passed or failed.\n\nBehavior:\n{{input}}",
  },
  {
    name: "docs",
    title: "Docs",
    description: "Write or update documentation with examples and verification notes.",
    action: "prompt",
    promptTemplate:
      "Write or update documentation for this. Keep it accurate to the current implementation and include concise examples where useful.\n\nTopic:\n{{input}}",
  },
  {
    name: "research",
    title: "Research",
    description: "Gather evidence, compare sources, and call out uncertainty.",
    action: "prompt",
    promptTemplate:
      "Research and verify this question. Use local context first when relevant, cite concrete evidence, and separate confirmed facts from inference.\n\nQuestion:\n{{input}}",
  },
  {
    name: "summarize",
    title: "Summarize",
    description: "Condense material into decisions, action items, risks, and open questions.",
    action: "prompt",
    promptTemplate:
      "Summarize this material into the key points, decisions, action items, risks, and open questions.\n\nMaterial:\n{{input}}",
  },
  {
    name: "compare",
    title: "Compare",
    description: "Compare options or artifacts with tradeoffs and a recommendation.",
    action: "prompt",
    promptTemplate:
      "Compare these options. Use a compact table when helpful, then recommend a path and explain the tradeoffs.\n\nOptions:\n{{input}}",
  },
  {
    name: "tasks",
    title: "Tasks",
    description: "Break work into independently shippable tasks.",
    action: "prompt",
    promptTemplate:
      "Break this into independently shippable tasks. Each task should have a clear outcome, acceptance checks, and dependencies.\n\nWork:\n{{input}}",
  },
  {
    name: "commit",
    title: "Commit",
    description: "Inspect current changes and draft a high-signal commit summary.",
    action: "prompt",
    promptTemplate:
      "Inspect the current changes and draft a precise commit summary. Include verification status and any risk that should be mentioned.\n\nContext:\n{{input}}",
  },
  {
    name: "image",
    title: "Image",
    description: "Create or edit visual assets with clear art direction.",
    action: "prompt",
    promptTemplate:
      "Create an image for this request. Ask only if required details are missing; otherwise choose a coherent visual direction and produce the asset.\n\nRequest:\n{{input}}",
  },
  {
    name: "skills",
    title: "Skills",
    description: "List or recommend relevant enabled skills for the task.",
    action: "prompt",
    promptTemplate:
      "Inspect the enabled skills and recommend which should be used for this task. If one is clearly relevant, use it directly.\n\nTask:\n{{input}}",
  },
  {
    name: "workflow",
    title: "Workflows",
    description: "Open the workflow catalog or select a workflow template.",
    action: "openWorkflows",
  },
  {
    name: "compact",
    title: "Compact",
    description: "Compact the current conversation context.",
    action: "compact",
  },
  {
    name: "pets",
    title: "Desktop Pets",
    description: "Control the local Desktop Pet without sending a message to the model.",
    action: "companion",
  },
  {
    name: "pet",
    title: "Desktop Pet (alias)",
    description: "Alias for /pets; accepts show, hide, settings, reset, or select <id>.",
    action: "companion",
  },
];

function normalizeSearch(value: string): string {
  return value.trim().toLowerCase().replace(/\s+/g, " ");
}

function stripBuiltinPrefix(value: string): string {
  return value.startsWith("builtin-") ? value.slice("builtin-".length) : value;
}

function commandSafeSlug(value: string): string {
  const slug = stripBuiltinPrefix(value)
    .trim()
    .toLowerCase()
    .replace(/[^a-z0-9_.:@-]+/g, "-")
    .replace(/^-+|-+$/g, "");
  return slug || "skill";
}

function displaySkillName(skill: Skill): string {
  const display = skill.interface?.displayName?.trim();
  return display || skill.name.trim() || skill.id;
}

function shortSkillDescription(skill: Skill): string {
  const short = skill.interface?.shortDescription?.trim();
  return short || skill.description.trim() || "Use this skill for its matching workflow.";
}

function makeSearchText(parts: Array<string | undefined | null>): string {
  return normalizeSearch(parts.filter(Boolean).join(" "));
}

export function getSlashCommandTrigger(text: string, cursorPosition: number): SlashCommandTrigger | null {
  const beforeCursor = text.slice(0, cursorPosition);
  const slashIndex = beforeCursor.lastIndexOf("/");
  if (slashIndex < 0) return null;

  const charBeforeSlash = slashIndex > 0 ? beforeCursor[slashIndex - 1] : "";
  if (charBeforeSlash && !/\s/.test(charBeforeSlash)) return null;

  const query = beforeCursor.slice(slashIndex + 1);
  if (/\s/.test(query)) return null;

  const textBeforeCurrentSlash = text.slice(0, slashIndex);
  if (new RegExp(`(^|\\s)/${COMMAND_NAME_PATTERN}\\s`).test(textBeforeCurrentSlash)) {
    return null;
  }

  return {
    start: slashIndex,
    end: cursorPosition,
    query,
    token: beforeCursor.slice(slashIndex, cursorPosition),
  };
}

export function buildSlashCommandOptions(
  skills: Skill[],
  workflowTemplates: SlashWorkflowTemplate[],
): SlashCommandOption[] {
  const common = COMMON_COMMANDS.map((command): SlashCommandOption => ({
    ...command,
    id: `command:${command.name}`,
    kind: "command",
    sourceLabel: "Command",
    searchText: makeSearchText([command.name, command.title, command.description]),
  }));

  const reservedNames = new Set(common.map((command) => command.name));
  const usedNames = new Set(reservedNames);

  const skillOptions = skills
    .filter((skill) => skill.enabled)
    .map((skill): SlashCommandOption => {
      const title = displaySkillName(skill);
      const baseSlug = commandSafeSlug(title || skill.name || skill.id);
      const fallbackSlug = commandSafeSlug(skill.id);
      let name = baseSlug;
      if (usedNames.has(name)) {
        name = fallbackSlug !== baseSlug && !usedNames.has(fallbackSlug) ? fallbackSlug : `skill:${fallbackSlug}`;
      }
      usedNames.add(name);
      return {
        id: `skill:${skill.id}`,
        name,
        title,
        description: shortSkillDescription(skill),
        kind: "skill",
        action: "prompt",
        sourceLabel: skill.builtin ? "Built-in skill" : "User skill",
        skillId: skill.id,
        skillName: skill.name,
        promptTemplate:
          skill.interface?.defaultPrompt?.trim() ||
          `Use the ${title} skill for this request.\n\nTask:\n{{input}}`,
        searchText: makeSearchText([
          name,
          title,
          skill.name,
          skill.id,
          skill.description,
          skill.interface?.shortDescription,
          skill.sourcePath,
        ]),
      };
    });

  const workflowOptions = workflowTemplates.map((template): SlashCommandOption => ({
    id: `workflow:${template.id}`,
    name: `workflow:${commandSafeSlug(template.id)}`,
    title: template.label,
    description: template.description,
    kind: "workflow",
    action: "prompt",
    sourceLabel: "Workflow",
    workflowTemplateId: template.id,
    promptTemplate: template.promptTemplate,
    searchText: makeSearchText([template.id, template.label, template.description]),
  }));

  return [...common, ...skillOptions, ...workflowOptions];
}

function matchScore(option: SlashCommandOption, query: string): number {
  if (!query) {
    if (option.kind === "command") return 30;
    if (option.kind === "skill") return 20;
    return 10;
  }

  const normalizedQuery = normalizeSearch(query);
  const name = option.name.toLowerCase();
  const title = option.title.toLowerCase();
  const search = option.searchText;

  if (name === normalizedQuery) return 100;
  if (name.startsWith(normalizedQuery)) return 90;
  if (title.startsWith(normalizedQuery)) return 82;
  if (name.includes(normalizedQuery)) return 74;
  if (search.includes(normalizedQuery)) return 62;

  const queryParts = normalizedQuery.split(/[-_:.\s]+/).filter(Boolean);
  if (queryParts.length > 1 && queryParts.every((part) => search.includes(part))) {
    return 55;
  }

  return 0;
}

export function getMatchingSlashCommands(
  options: SlashCommandOption[],
  query: string,
  limit = 12,
): SlashCommandOption[] {
  return options
    .map((option) => ({ option, score: matchScore(option, query) }))
    .filter((entry) => entry.score > 0)
    .sort((a, b) => {
      if (b.score !== a.score) return b.score - a.score;
      if (a.option.kind !== b.option.kind) {
        return kindRank(a.option.kind) - kindRank(b.option.kind);
      }
      return a.option.name.localeCompare(b.option.name);
    })
    .slice(0, limit)
    .map((entry) => entry.option);
}

function kindRank(kind: SlashCommandKind): number {
  if (kind === "command") return 0;
  if (kind === "skill") return 1;
  return 2;
}

export function insertSlashCommand(
  text: string,
  trigger: SlashCommandTrigger,
  command: SlashCommandOption,
): { value: string; cursorPosition: number } {
  const before = text.slice(0, trigger.start);
  const after = text.slice(trigger.end);
  const inserted = `/${command.name}`;
  const needsSpace = !after.startsWith(" ");
  const value = `${before}${inserted}${needsSpace ? " " : ""}${after}`;
  return {
    value,
    cursorPosition: before.length + inserted.length + (needsSpace ? 1 : 0),
  };
}

function expandPromptTemplate(template: string | undefined, input: string): string {
  const trimmedInput = input.trim();
  const base = (template ?? "{{input}}").trimEnd();
  if (base.includes("{{input}}")) {
    return base.split("{{input}}").join(trimmedInput).trim();
  }
  return trimmedInput ? `${base}\n\n${trimmedInput}` : base.trim();
}

export function resolveSlashCommandMessage(
  message: string,
  options: SlashCommandOption[],
): ResolvedSlashCommand | null {
  const match = FIRST_COMMAND_RE.exec(message);
  if (!match) return null;

  const leadingWhitespace = match[1] ?? "";
  const commandName = match[2] ?? "";
  const option = options.find((candidate) => candidate.name.toLowerCase() === commandName.toLowerCase());
  if (!option) return null;

  const commandStart = match.index + leadingWhitespace.length;
  const commandEnd = commandStart + commandName.length + 1;
  const remainder = `${message.slice(0, commandStart)}${message.slice(commandEnd)}`.trim();

  return resolveSlashCommandSelection(option, remainder);
}

/** Resolve an already-selected command against the visible composer text. */
export function resolveSlashCommandSelection(
  option: SlashCommandOption,
  input: string,
): ResolvedSlashCommand {
  const remainder = input.trim();

  if (option.action === "compact" || option.action === "openWorkflows" || option.action === "companion") {
    return {
      command: option,
      message: remainder,
      skillIds: [],
      localAction: option.action,
      artifact: slashCommandArtifact(option),
    };
  }

  if (option.action === "planMode") {
    return {
      command: option,
      message: remainder,
      skillIds: [],
      executionMode: "plan",
      artifact: slashCommandArtifact(option),
    };
  }

  const skillIds = option.skillId ? [option.skillId] : [];
  const expandedMessage = expandPromptTemplate(option.promptTemplate, remainder);
  if (option.kind === "command" && option.name === "goal") {
    const objective = remainder || option.title;
    const clearsGoal = /^(clear|cancel|stop)$/i.test(remainder.trim());
    return {
      command: option,
      message: clearsGoal ? "Clear the active conversation goal." : expandedMessage,
      displayMessage: objective,
      skillIds,
      artifact: goalCommandArtifact(option, objective, clearsGoal ? "cleared" : "active"),
    };
  }

  return {
    command: option,
    message: option.kind === "workflow" && option.workflowTemplateId
      ? buildWorkflowBatchPrompt({ id: option.workflowTemplateId }, expandedMessage)
      : expandedMessage,
    skillIds,
    artifact: slashCommandArtifact(option),
  };
}

export function slashCommandArtifact(option: SlashCommandOption): Record<string, unknown> {
  return {
    kind: "slashCommand",
    command: option.name,
    commandKind: option.kind,
    title: option.title,
    skillId: option.skillId ?? null,
    workflowTemplateId: option.workflowTemplateId ?? null,
    executionMode: option.action === "planMode" ? "plan" : null,
  };
}

function goalCommandArtifact(
  option: SlashCommandOption,
  objective: string,
  status: "active" | "cleared",
): Record<string, unknown> {
  return {
    ...slashCommandArtifact(option),
    kind: "goal",
    version: 1,
    objective,
    status,
  };
}
