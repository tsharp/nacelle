# Performance and Edge Next Plan

**Date:** 2026-06-07

## Purpose

Track the next engineering pass after the raw TCP TLS, HTTP hardening, and stress harness work. This plan separates performance rebaselining from direct public edge hardening so that safety work does not hide unexplained throughput loss.

## Phase 1: Rebaseline Raw TCP Performance

- Re-run Linux raw TCP with `tls_self_signed = false`, `low_memory = true`, and the restored plain client owned split.
- Re-run Linux raw TCP with `low_memory = false` only as a comparison point, not as the primary suspected cause.
- Capture exact commit, CPU governor, kernel, allocator env vars, config, payload, pipeline, connection count, and thread count.
- Compare against commit `b7b3042` and current `feat/edge` with the same harness.

Acceptance:

- Plain raw TCP benchmark is either back near the 1.9M RPS baseline or the remaining delta is attributed to a specific server-side hardening cost.
- Benchmark notes live in `docs/internal` with exact command lines and metrics.

## Phase 2: Add Repeatable Perf Profiles

- Add checked-in stress configs for raw TCP, raw TCP low-memory, and raw TCP TLS.
- Update `run-tokio.sh` and `run-tokio.ps1` to accept a config path so profiles can be selected without editing root config.
- Keep local defaults convenient, but make benchmark defaults explicit.

Acceptance:

- A contributor can run the documented raw TCP baseline without editing `config.toml`.
- TLS and non-TLS runs cannot accidentally use mismatched client/server modes.

## Phase 3: Measure Server-Side Hardening Costs

- Isolate per-request permit accounting.
- Isolate timeout wrapper overhead.
- Isolate memory reservation accounting.
- Isolate telemetry disabled-path overhead.
- Add Criterion microbenchmarks for the isolated primitives where the results are stable enough to be useful.

Acceptance:

- Each hot-path guard has a measured cost or is shown below noise for the benchmark host.
- Any optimization preserves production limits and validation coverage.

## Phase 4: Direct Edge Hardening

- Add TLS SNI allowlist support.
- Document Host/SNI mismatch behavior.
- Add accept-rate and connection-churn protection.
- Add proxy identity handling only behind explicit trusted proxy configuration.
- Expand adversarial tests for malformed, slow, high-churn, and TLS-handshake failure cases.

Acceptance:

- Direct public edge readiness plan gates are updated with implemented status.
- New controls are opt-in where they add measurable hot-path overhead.

