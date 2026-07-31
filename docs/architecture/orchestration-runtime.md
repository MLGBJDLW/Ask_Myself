# Orchestration Runtime

This document records the runtime contracts behind Nexa's Mixture-of-Agents
(MoA), Nexus, and orchestration quality profiles. The implementation is based
on primary-source review performed on 2026-07-31; it is not a claim of wire or
configuration compatibility with another project.

## Design sources

- [Together Mixture-of-Agents paper](https://arxiv.org/abs/2406.04692) and
  [reference implementation](https://github.com/togethercomputer/moa) establish
  the parallel reference-model and acting-aggregator pattern.
- [Hermes Agent `moa_loop.py`](https://github.com/NousResearch/hermes-agent/blob/main/agent/moa_loop.py)
  demonstrates production constraints that the original paper does not cover:
  advisors are tool-free, fan-out is bounded, partial advisor failure is not
  fatal, private reference context is filtered, and usage is attributed to the
  model that incurred it. The current upstream release at review time was
  [v0.19.1](https://github.com/NousResearch/hermes-agent/releases/tag/v2026.7.30).
- [Anthropic's orchestrator-workers and evaluator-optimizer patterns](https://www.anthropic.com/engineering/building-effective-agents)
  motivate independent parallel reconnaissance followed by explicit synthesis
  and verification.
- [Magentic-One](https://github.com/microsoft/autogen/tree/main/python/packages/autogen-magentic-one)
  provides a useful reference for an orchestrator-led multi-agent team with
  specialized workers.
- [LangGraph checkpointing](https://github.com/langchain-ai/langgraph/tree/main/libs/checkpoint)
  informed the decision to make workflow progress serializable rather than
  keeping scheduler state only in a prompt.

## Product contracts

MoA and Nexus are independent axes. MoA changes how an acting model receives
advice. Nexus changes how the client plans, delegates, checkpoints, and verifies
work. Either may be enabled alone or both may be composed.

Provider reasoning effort remains a third, independent axis. `Code Ultra` and
`Research Ultra` are Nexa orchestration profiles; they must never be sent to a
provider as invented reasoning-effort values.

High-cost behavior is explicit and per conversation. The composer shows when
MoA or a non-balanced profile is active, explains the added calls and token
cost, and preserves an off/balanced path.

## MoA execution boundary

`MoaProvider` wraps the acting provider without changing the provider contract.
On an eligible cadence it:

1. Builds a deterministic, privacy-filtered advisor view of the turn.
2. Calls configured advisor providers concurrently with no tools.
3. Keeps successful advice when another advisor fails.
4. Appends private labelled advice to the acting aggregator's context.
5. Lets only the aggregator use tools, stream user-visible output, and own turn
   termination.
6. Aggregates normalized usage while retaining per-slot model and reasoning
   settings.

The built-in presets (`Fast Review`, `Deep Research`, and `Cross-model Code
Review`) are shortcuts over this contract. `Custom` remains bounded by the same
fan-out, cadence, privacy, and token-reserve controls.

## Workflow IR and Nexus execution

Nexus compiles the typed task plan into versioned `WorkflowIr` before the first
model request. The IR is a validated DAG containing dependencies, parallel
groups, model-routing classes, tool policy, write isolation, retry policy,
structured deliverables, an evidence ledger, checkpoints, verification gates,
and a completion contract.

For non-trivial Nexus turns, the runtime automatically dispatches the first two
ready reconnaissance nodes through `spawn_subagent_batch`. This is a controller
action, not prompt advice. The wave is read-only, each worker has a stable node
id, and worker results update sibling nodes independently. The controller keeps
dispatching any retryable failed node until it succeeds or exhausts its node
retry policy, without discarding successful branches.

Each delegated node carries its model-routing class into the executor. `Fast`
and `IndependentReviewer` use the configured auxiliary model only when that
model belongs to the same provider endpoint as the parent; otherwise the run
records an explicit fallback and keeps the parent model. `Strong` always keeps
the parent model. This provider-compatibility guard prevents cross-provider
model names from being sent with the wrong endpoint or credentials.

The acting agent receives the updated IR and owns mutation and synthesis.
`Code Ultra` additionally requires isolated writes plus test, typecheck, build,
and independent-review gates. `Research Ultra` raises evidence and independent
verification requirements without forcing a code workspace.

For a mutation-capable Code Ultra workflow, the controller requires exactly one
clean Git-backed source, creates a detached temporary worktree, registers it as
a non-watched source for the turn, scopes execution only to that source, and
rewrites filesystem paths, exact shell argv repository paths, and shell working
directories into it while rejecting outside or traversing paths. Free-form shell
commands, shell interpreters, inline interpreter code, and `project_tool run` are
withheld because they cannot preserve this routing contract. Only after every
other required gate passes does the controller generate a binary Git patch,
verify it with `git apply --check`,
promote it to the original clean worktree, and remove the temporary source. The
write-isolation gate is set only by this runtime transition; a model-authored
`record_verification` label cannot satisfy it. Likewise, independent review is
derived only from a successful `subagent_judgement` runtime artifact.

## Evaluation contract

The `orchestration_runtime` evaluation suite must cover:

- valid DAG construction and parallel readiness;
- structured artifacts, isolated writes, checkpoints, and affected-node retry;
- bounded MoA fan-out and advisor failure fallback;
- the independent four-state matrix for Nexus and MoA;
- strict separation of client orchestration and provider reasoning effort;
- comparison metrics for first-pass completion, tests, regressions, verifier
  true positives, correction rounds, wall time, tokens, estimated cost, and Nexus net
  improvement.

Runtime quality claims should be made only after comparing the relevant profile
against the balanced direct baseline on the same task set.
