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
   across retries and process restarts. Its origin distinguishes a recurrence
   from an explicit Workbench `Run now` request.
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

`Run now` materializes a `manual_run_now` occurrence through the same atomic
claim, approval, lease, and retry path. It snapshots the recurring cursor and
never consumes or shifts `next_run_at`; approval, denial, completion, and retry
restore that cursor. `Due now` remains a real schedule-origin occurrence.

Every saved schedule revision has an immutable definition snapshot. Editing a
definition explicitly cancels any planned, retrying, or approval-waiting
occurrence from the superseded revision and records that decision; an old run is
never silently orphaned by a query that only sees the new revision.

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
narrowest tool set that can complete the task. An empty allowed-tools list means
no tools; it never inherits the unrestricted root registry. Approval-required
definitions create a durable occurrence and `waiting_approval` run. Workbench
shows explicit Approve and Deny actions for every waiting occurrence, including
multiple occurrences of the same overlap-allow definition. The decision remains
bound to that occurrence across restart, retry, and lease recovery. A minute
poll observes the definition as non-due while that run waits rather than
appending duplicate "skipped" records.

After the occurrence itself is approved (or when pre-run approval is disabled),
the immutable saved allowlist becomes an audited per-run grant. This bypasses a
second generic `Ask` prompt only inside the already narrowed scheduled registry,
so unattended file/shell adapters do not time out waiting for a hidden window.
Hard computer-control, screen-disclosure, and interactive browser confirmations
remain non-bypassable and are not accepted as unattended scheduled tools.

The optional Project binding owns project instructions, memory, and source
membership. The exact source IDs are snapshotted into the definition. Reusing a
conversation from another Project, removing a Source from the Project, or
repointing the Source root fails closed at launch.

Workspace writes have a separate policy:

- `deny_writes` is the default. Only the reviewed unattended read/network tool
  set is accepted; unknown, MCP, Office, project, browser-control, and desktop
  mutators fail closed. Read-only Nexus delegation is allowed because workers
  inherit the same narrowed root registry.
- `isolated_patch` accepts only the built-in `create_file`, `edit_file`,
  `multi_edit`, and constrained `run_shell` mutation adapters. It requires one
  explicit Source, Code Ultra, normal execution, and overlap `skip`. The
  controller always creates an isolated Git worktree, even if planner inference
  describes the task as read-only, and promotes only a verified patch from the
  current clean `HEAD`. Before promotion, the controller performs a
  non-delegating Git patch review (`diff --check` plus clean-HEAD applicability)
  and records the independent-review gate. Delegation is rejected in this mode
  until worker runtimes can inherit the same filesystem sandbox.

The Source root fingerprint is checked again at launch, and active isolated
patches hold a database claim lock on the canonical root fingerprint across
automation definitions, including two Source IDs that alias the same directory.
Normal termination cleans the temporary worktree immediately. Restart recovery
atomically terminalizes interrupted Agent Run, Workflow Run, and occurrence,
releases the source lock, and reconciles a durable isolation-ownership ledger.
The ownership intent is committed before `git worktree add`, then bound to its
temporary Source. UUID/Git ownership must verify and worktree removal must
succeed before the Source or ledger is deleted. Paused, awaiting-input, and
resuming owners keep their worktree for continuation; unverifiable or legacy
temp entries retain both directory and Source rather than being deleted
blindly. The runtime does not promise a selectable base ref or OS notification.

Workbench Run now, Due now, public desktop commands, and background scheduler
ticks share the same backend launch seam. None may replace the saved route,
context, Nexus/profile, approval, or root tool policy by handing the prompt to a
generic Chat launch. The command result is explicit: `launched`,
`pending_approval`, or `skipped`, so both Workbench entry points refresh the
same durable state instead of presenting approval as an error.

This unattended grant does not leak into an ordinary user-started, non-schedule
workflow. Interactive workflow runs keep the normal per-tool `Ask` policy, and
an empty allowlist keeps the ordinary eligible registry. Scheduled execution is
the only path where an empty saved allowlist intentionally means zero tools.

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
5. For repository writes, select exactly one Git-backed Source and the isolated
   patch policy; keep the normal checkout clean so promotion can be verified.
6. Review the first runs in Workbench or Task Center, then tune cadence, prompt,
   and retry behavior. Pause a definition that needs credentials or migration.

Local scheduled tasks require the computer and Nexa desktop runtime to remain
available. Definitions using `deny_writes` cannot mutate a checkout; definitions
using `isolated_patch` use the controller-owned worktree contract above.

## Compatibility

Legacy `{ kind: "schedule", cron }` definitions remain readable. Safe daily UTC
definitions can be upgraded without changing their observed cadence. Legacy
expressions whose previously ignored fields would change meaning must be paused
for review instead of silently acquiring new cadence. `next_run_at` is a derived
cache; the occurrence record is the authority for claiming and retrying work.
