# Scheduled Tasks

Scheduled Tasks are durable workflow definitions hosted by the Nexa desktop
runtime. They reuse the normal Agent Run, provider, tool, approval, event, and
task-projection paths; scheduling is not a second agent runtime.

## Design sources

- [ChatGPT and Codex Scheduled Tasks](https://learn.chatgpt.com/docs/automations)
  informs default model inheritance, project/worktree execution, unattended
  permissions, run review, and the long-term RFC 5545 direction.
- [RFC 5545](https://www.rfc-editor.org/rfc/rfc5545) defines recurrence and
  timezone concepts used by that long-term interoperable direction.
- [Kubernetes CronJob](https://kubernetes.io/docs/concepts/workloads/controllers/cron-jobs/)
  informs explicit timezones, missed-start deadlines, overlap policy,
  suspension, stable scheduled timestamps, and idempotent jobs.
- [`cronexpr`](https://github.com/fast/cronexpr) is the pinned Cron5/IANA
  recurrence engine. Nexa owns validation, persistence, occurrence identity,
  retry, and presentation around it.

## Runtime contract

A scheduled task has three distinct records:

1. The definition stores the prompt, schedule, timezone, execution policy, tool
   policy, and retry/overlap behavior.
2. An occurrence identifies one intended fire time. Its identity is stable
   across retries and process restarts.
3. An Agent Run records one execution attempt and its durable events, output,
   usage, and terminal state.

The scheduler validates a definition before enabling it, previews upcoming
occurrences in its named IANA timezone, and claims an occurrence atomically.
Concurrent ticks must not launch the same occurrence twice. Each claim carries
the definition revision and a short lease; starting an attempt verifies that it
is still the occurrence's authoritative leased attempt. An expired claimant is
fenced out after a replacement attempt is created. A launch failure retries the
same occurrence rather than advancing the definition as if the work had run
successfully.

Misfire policy applies only before an occurrence has been attempted. Once an
attempt enters retry backoff, it remains retryable even after the original grace
window. `run_latest` materializes the last missed occurrence at or before the
current instant; `skip` records the missed occurrence without running it.

The current advanced schedule contract is a complete five-field cron expression
plus an IANA timezone. Lists, ranges, steps, day-of-month, month, and day-of-week
are real schedule fields; no field may be accepted and then ignored. The UI must
show the timezone and upcoming occurrences before save. RFC 5545 recurrence is a
future-compatible direction, not an accepted syntax until the runtime can parse,
preview, migrate, and execute it with the same guarantees.

## Execution policy

By default, model, reasoning, and context remain automatic and are resolved when
the occurrence launches. A task can instead pin an Agent configuration and
explicit execution choices when reproducibility matters. An empty context
override uses the selected provider route's default capacity; an unknown custom
route remains provider-managed rather than receiving a fabricated limit.

Scheduled tasks run unattended. Tool restrictions are enforced by the runtime
registry used for that run, not merely written into the prompt. Configure the
narrowest tool set that can complete the task. Approval-required definitions
must create one actionable waiting state; a minute poll must not append duplicate
"skipped" events indefinitely.

Workbench Run now, Due now, public desktop commands, and background scheduler
ticks share the same backend launch seam. None may replace the saved route,
context, Nexus/profile, approval, or root tool policy by handing the prompt to a
generic Chat launch.

## User workflow

Before enabling a recurring task:

1. Run the prompt manually with the intended project, model defaults, skills,
   and tool access.
2. Choose the business timezone, then verify the preview across the next several
   occurrences. For daylight-saving zones, inspect a transition date when it is
   relevant.
3. Leave the model and context on Auto for provider-managed evolution, or pin an
   Agent configuration when repeatability outweighs automatic upgrades.
4. Start with least-privilege tools and a conservative overlap policy.
5. Review the first runs, then tune cadence, prompt, retry, and notification
   behavior. Pause a definition that needs credentials, approval, or migration.

Local scheduled tasks require the computer and Nexa desktop runtime to remain
available. A Git worktree isolation mode is the preferred future host for tasks
that mutate repositories; until a task explicitly records and enforces that
mode, users should not assume scheduled edits are isolated from their checkout.

## Compatibility

Legacy `{ kind: "schedule", cron }` definitions remain readable. Safe daily UTC
definitions can be upgraded without changing their observed cadence. Legacy
expressions whose previously ignored fields would change meaning must be paused
for review instead of silently acquiring new cadence. `next_run_at` is a derived
cache; the occurrence record is the authority for claiming and retrying work.
