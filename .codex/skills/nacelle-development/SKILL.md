---
name: nacelle-development
description: Implement nacelle code, docs, tests, and performance or edge-hardening changes. Use when Codex is making repository changes in nacelle, especially Rust crate changes, stress harness work, TLS/HTTP/raw TCP behavior, mdBook docs, or CI fixes.
---

# Nacelle Development

Use this skill when changing the nacelle repository.

## Development Principles

- Read the surrounding code before editing. Follow existing crate boundaries and public API style.
- Keep `nacelle-core` transport-neutral. Put raw TCP behavior in `nacelle-tcp`, HTTP behavior in `nacelle-http`, and umbrella re-exports/reference protocol in `nacelle`.
- Keep TLS shared and transport-neutral unless a behavior is truly HTTP-only or raw-TCP-only.
- Keep public Markdown documentation in the mdBook source tree under `docs/`; keep plans and assessments in `docs/internal/`.
- Do not reintroduce DocFX. mdBook is the canonical Markdown site generator; `cargo doc` is the Rust API reference path.
- Preserve benchmarkability. Use explicit stress profiles rather than editing root config for comparisons.
- Avoid hot-path overhead by default. Add opt-in policy for direct-edge controls where possible.
- Use `apply_patch` for manual edits.

## Implementation Workflow

1. Check state:
   - `git status --short --branch`
   - `git log --oneline -3`
2. Read relevant files in parallel where possible:
   - crate manifests
   - implementation modules
   - tests
   - docs/internal plan
   - public docs affected by the change
3. Make the smallest coherent change.
4. Add or update tests where behavior changes.
5. Update docs when changing operator-facing behavior, config, public APIs, or readiness posture.
6. Run focused validation first.
7. Run broader validation before finishing or committing.
8. Commit meaningful checkpoints when requested.

## Validation Matrix

Use the narrowest useful command first, then broaden.

For formatting:

```powershell
cargo fmt --all
```

For general compile surface:

```powershell
cargo check --workspace --all-features
```

For CI-style lint:

```powershell
cargo clippy --workspace --all-targets --all-features -- -D warnings
```

For full tests:

```powershell
cargo test --workspace --all-features
```

For security:

```powershell
cargo audit
```

For docs:

```powershell
scripts\build-book.ps1
scripts\build-rustdoc.ps1
```

For performance microbench compile:

```powershell
cargo bench --package nacelle --features bench,reference_protocol --bench critical_paths --no-run
```

For Linux RPS comparison, use the explicit stress profiles:

```bash
./build-all.sh
./run-tokio.sh --config nacelle-stress-server/configs/raw-tcp.toml --server-threads 48 --connections 256 --pipeline 8 --duration-secs 30 --payload-bytes 256
./run-tokio.sh --config nacelle-stress-server/configs/raw-tcp-low-memory.toml --server-threads 48 --connections 256 --pipeline 8 --duration-secs 30 --payload-bytes 256
./run-tokio.sh --config nacelle-stress-server/configs/raw-tcp-tls.toml --server-threads 48 --connections 256 --pipeline 8 --duration-secs 30 --payload-bytes 256
```

## Hot-Path Rules

- Do not compare TLS and non-TLS throughput as the same path.
- Keep server stats disabled for peak RPS runs unless validating counters.
- Keep telemetry sinks optional; default operation should not push into in-memory sinks.
- Keep per-request locks out of the raw TCP path.
- Measure new guardrails with Criterion when stable enough to be useful.
- Rebaseline on Linux for throughput claims; Windows checks are compile/test confidence, not RPS evidence.

## Edge-Hardening Rules

- Enforce Host policy in HTTP with `NacelleHttpPolicy`.
- Enforce SNI policy in shared TLS config with `NacelleTlsConfig`.
- Keep forwarded peer headers ignored unless `with_trusted_proxy_ips(...)` explicitly trusts the socket peer.
- Use low-cardinality telemetry reasons for rejection/timeout events.
- Prefer fast rejection over queue growth when limits are exceeded.

## Commit Checkpoints

When checkpoint commits are requested:

1. Stage only the completed coherent slice.
2. Run validation appropriate to that slice.
3. Commit with a short conventional message, for example:
   - `docs: ...`
   - `feat: ...`
   - `fix: ...`
   - `perf: ...`
   - `bench: ...`
4. Re-check `git status --short --branch` before continuing.
