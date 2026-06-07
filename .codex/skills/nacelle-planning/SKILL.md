---
name: nacelle-planning
description: Plan nacelle work before implementation. Use when Codex needs to create or update plans for production readiness, edge hardening, performance work, crate architecture, documentation structure, release readiness, or multi-step nacelle engineering tasks.
---

# Nacelle Planning

Use this skill to turn broad nacelle goals into actionable, traceable plans.

## Planning Principles

- Start from the repository state, not memory. Read existing docs, plans, tests, configuration, crate layout, and recent commits before proposing work.
- Separate production safety from benchmark performance so hardening work does not hide unexplained throughput loss.
- Prefer explicit gates and acceptance criteria over vague readiness claims.
- Keep `docs/internal/` for plans, assessments, and decision records. Keep public usage material in the mdBook source tree under `docs/`.
- Use dates in internal plan filenames when the plan captures a time-bound assessment or execution pass.
- Preserve raw TCP hot-path performance as a first-class constraint when adding edge controls.
- Make edge controls opt-in when they add measurable overhead or are only relevant for public traffic.
- Treat "production ready" as deployment-scope dependent:
  - controlled/internal or proxy-protected deployment
  - direct public edge deployment
  - local load testing/autodeploy use

## Planning Workflow

1. Inspect the current state:
   - `git status --short --branch`
   - relevant files under `docs/internal/`
   - crate manifests and touched modules
   - current config and benchmark scripts
   - relevant tests and CI workflow
2. Identify the deployment or performance target.
3. Split work into phases with acceptance criteria.
4. Mark dependencies and order:
   - docs or plan first when the work changes architecture or readiness posture
   - low-risk tooling/profile changes before behavioral changes
   - hot-path measurement before optimization
   - edge hardening behind explicit configuration
5. Define validation for each phase:
   - focused unit/integration tests
   - `cargo check --workspace --all-features`
   - `cargo clippy --workspace --all-targets --all-features -- -D warnings`
   - `cargo test --workspace --all-features`
   - `cargo audit`
   - `scripts/build-book.ps1`
   - Linux stress runs for RPS or soak claims
6. Update the relevant internal plan as implementation proceeds.

## Plan Shape

Use this structure for substantial plans:

```markdown
# <Topic> Plan

**Date:** YYYY-MM-DD

## Purpose

## Phase 1: <Name>

- task
- task

Acceptance:

- measurable gate
- validation gate
```

## Readiness Language

Avoid saying "production ready" without scope. Prefer:

- "production-capable for controlled/proxy-protected deployments"
- "not yet direct public edge ready"
- "direct-edge beta after these gates pass"
- "requires Linux perf rebaseline before merging/releasing"

## Checkpoint Strategy

When the user asks for checkpoint commits, plan commits around meaningful slices:

- docs/plan checkpoint
- crate/API split checkpoint
- TLS or edge-control checkpoint
- stress harness/perf profile checkpoint
- validation or CI-fix checkpoint

Each checkpoint should build or test at the right level for its risk.
