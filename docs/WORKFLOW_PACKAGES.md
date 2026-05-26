# Workflow Packages

Workflow packages are user-facing task templates. They compose tools, skills,
connectors, agent roles, prompts, and approval expectations into repeatable
workflows that non-technical users can understand.

Workflows are not native plugins. They should not contain host code. They are
product contracts that tell Nexa how to guide a task.

## Package Shape

The long-term portable shape is:

```text
<workflow-id>/
  workflow.yaml
  prompts/
  examples/
  tests/
```

The core workflow manifest should describe:

```yaml
id: document_compare
name: Document Compare
surface: workflow_package
description: Compare local documents for overlap, contradictions, and decision-relevant differences.
version: 1
requiredTools:
  - compare_documents
  - retrieve_evidence
optionalSkills:
  - evidence-first
approval:
  writesFiles: false
  usesNetwork: false
```

## Workflow Rules

Workflow packages should:

- use consumer-facing language
- make source scope and evidence expectations explicit
- list required tools and optional skills
- avoid raw chain-of-thought or model-debug wording
- describe review, cancellation, and failure states for long tasks
- declare whether file writes, network access, shell execution, or connector
  tools may be needed

## Built-In Status

Nexa's built-in workflow catalog is represented by the `builtin-workflows`
package manifest with surface `workflow_package`. Each catalog template maps to:

```text
.nexa/capabilities/builtin-workflows/workflows/<workflow-id>/workflow.yaml
```

Current built-in workflows include:

- Research + Verify
- Draft + Review
- Meeting Summary
- Document Compare
- Report Brief
- Connector + Background Task

These workflows should remain product-facing. If a workflow needs new external
tools, it should declare connector dependencies instead of becoming a native
plugin.

Project-local workflow package manifests can use the same
`.nexa/capabilities/*/capability.yaml` discovery path as other capability
packages.
