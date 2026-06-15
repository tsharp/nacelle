---
name: development
description: Implement repository changes in any software project. Use when Codex changes code, tests, docs, configs, CI, build scripts, performance-sensitive paths, security hardening, migrations, release preparation, or checkpoint commits while preserving the project's existing architecture and validation style.
---

# Project Development

Make small, validated changes that respect the repository's conventions, ownership boundaries, and risk profile.

## Core Rules

- Inspect state first: `git status --short --branch` and `git log --oneline -3`.
- Read surrounding code, tests, manifests, configs, docs, CI, and existing plans before editing.
- Use `project-planning` first for broad, risky, architectural, release, readiness, migration, or multi-session work.
- Infer module, package, crate, service, layer, and public/private boundaries from the codebase; keep changes inside the smallest fitting boundary.
- Preserve public APIs, schemas, config defaults, persisted data formats, and user-facing behavior unless the request requires changing them.
- Let the project's package manager update lockfiles only when dependencies change.
- Protect performance-sensitive paths by default; add measurement, feature gates, batching, caching, or opt-in controls when new work could add steady-state overhead.
- Keep generated files, formatting churn, dependency bumps, and metadata edits out of scope unless needed for the task.
- Use `apply_patch` for manual edits.

## Workflow

1. Read relevant files in parallel when possible: manifests, implementation modules, tests, scripts, CI, plans, and affected docs.
2. Identify the local pattern before choosing an implementation style.
3. Make the smallest coherent change that solves the request.
4. Add or update tests for behavior changes, regressions, migrations, or contracts.
5. Update docs for operator behavior, config, public APIs, workflows, compatibility, or readiness posture.
6. Run focused validation first, then broaden to match the blast radius.
7. For requested checkpoints, stage one coherent slice, validate it, commit with the repository's message style, and re-check status.

## Validation

Prefer repo-provided commands from scripts, CI, task runners, or docs. If the project does not make validation obvious, choose the narrowest useful proof:

- Rust: `cargo fmt --all`; `cargo check --workspace --all-features`; `cargo clippy --workspace --all-targets --all-features -- -D warnings`; `cargo test --workspace --all-features`
- Node/TypeScript: use the detected package manager, then run available `lint`, `typecheck`, `test`, and `build` scripts.
- Python: `python -m pytest` plus configured format, lint, or type checks such as `ruff`, `black --check`, or `mypy`.
- Go: `gofmt`, `go test ./...`, and configured lint or vet commands.
- .NET: `dotnet format --verify-no-changes`, `dotnet build`, and `dotnet test` when available.
- Docs/config only: run the docs build, schema validation, generated-doc check, or link/check script if the repo provides one.

## Performance-Sensitive Work

- Establish the existing baseline before making throughput, latency, memory, or size claims.
- Compare like with like: same platform, feature flags, config, workload, dataset, cache state, and security mode.
- Keep counters, tracing, logging, allocation-heavy helpers, and per-request locks out of hot paths unless measured and justified.
- Prefer explicit benchmark or stress profiles over changing default/root config for comparisons.
- Treat local development measurements as confidence checks, not release-grade performance proof, unless the repo already accepts them.

## Security And Hardening

- Check trust boundaries, input validation, authentication, authorization, secrets, transport security, dependency risk, and unsafe/deserialization behavior when touched.
- Make stricter public-edge or production controls explicit, documented, and configurable when compatibility or performance could be affected.
- Use low-cardinality telemetry and stable error reasons for rejection, timeout, rate-limit, or policy events.
- Prefer fast rejection, bounded queues, timeouts, and backpressure over unbounded resource growth.

## Checkpoint Commits

Use https://www.conventionalcommits.org commit style. If no style is evident, use short conventional messages such as `docs: ...`, `feat: ...`, `fix: ...`, `perf: ...`, `test: ...`, or `build: ...`.
