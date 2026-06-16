---
name: planning
description: Plan repository work before implementation in any software project. Use when Codex needs to scope broad or risky work, architecture changes, migrations, production/readiness posture, security hardening, performance baselines, release gates, documentation structure, validation strategy, or checkpoint commits across one or more sessions.
---

# Project Planning

Turn broad goals into scoped, ordered work with explicit risks, validation gates, and handoff-ready checkpoints.

## Core Rules

- Inspect the repo first: `git status --short --branch`, relevant plans, manifests, modules, tests, configs, scripts, CI, docs, and recent commits.
- Use chat for narrow plans. For broad, risky, architectural, release, migration, or multi-session work, write or update a durable plan using the repo's existing plan location.
- If no plan convention exists, prefer `docs/internal/YYYY-MM-DD-<topic>-plan.md` for project-owned internal plans, or `.codex/plans/YYYY-MM-DD-<topic>-plan.md` for agent-only working plans.
- Continue the nearest existing plan when it matches the goal; create a dated file for new assessments, execution passes, or materially different scope.
- Keep internal assessments and decisions separate from public operator, API, user, or release docs.
- Leave code-level tactics to `project-development` unless they affect ordering, risk, validation, or architecture.
- Scope readiness claims by environment, user group, traffic level, data sensitivity, support expectations, and rollback posture.
- Separate correctness, security, performance, operability, compatibility, and documentation gates; one gate does not prove the others.
- Skip generic checklists; include task-specific risks, evidence, commands, owners or handoff points, and stop conditions.

## Workflow

1. Inspect current state and any existing plan.
2. Name the target: feature, bug class, migration, architecture change, release, readiness level, performance claim, docs deliverable, or checkpoint series.
3. State non-goals and constraints so the plan does not absorb unrelated refactors.
4. Phase the work with goals, tasks, dependencies, acceptance criteria, validation evidence, and rollback or mitigation where relevant.
5. Order by risk: discovery before design, tests before risky behavior changes, compatibility before migration, measurement before optimization, docs before public readiness claims.
6. Record risks, unknowns, and the first measurement or inspection that will reduce each one.
7. Update the plan when facts change; preserve useful decisions and mark superseded assumptions clearly.

## Durable Plan Format

For substantial plans, include:

- purpose and scope
- target users, environments, or release/readiness level
- non-goals
- current state
- ordered phases
- acceptance and validation gates
- risks, unknowns, and mitigations
- checkpoint or handoff slices

## Validation Planning

Choose the smallest proof that actually supports the claim:

- focused unit, integration, regression, migration, contract, or end-to-end tests
- format, lint, typecheck, compile, build, package, docs, and schema validation commands from the repo
- security checks such as dependency audit, secret scanning, authz/authn regression tests, fuzzing, or threat-model review when relevant
- performance or soak runs with exact environment, workload, config, dataset, baseline, and comparison criteria
- operational proof such as observability, alerting, rollback, data recovery, rate limits, and runbook updates

## Readiness Language

Do not say "production ready" without scope. Prefer concrete claims such as:

- "ready for internal users after these gates pass"
- "safe for beta behind a feature flag"
- "ready for low-risk production traffic with rollback documented"
- "not yet ready for public or regulated workloads"
- "requires performance rebaseline before release"

## Checkpoint Commits

When checkpoint commits are requested, plan coherent slices such as docs/plan, tests, architecture boundary, behavior change, migration, security control, performance profile, validation/CI, or release prep. Each checkpoint should build or test at the level required by its risk.

Use https://www.conventionalcommits.org commit style.
