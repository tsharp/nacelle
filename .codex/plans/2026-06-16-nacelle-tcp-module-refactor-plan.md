# nacelle-tcp Module Refactor Plan

## Purpose And Scope

Split oversized `nacelle-tcp` source files into smaller responsibility-focused modules while preserving public APIs and runtime behavior.

## Non-Goals

- Do not change protocol behavior, connection lifecycle behavior, telemetry labels, timeouts, or default configuration.
- Do not add dependencies or change Cargo features.
- Do not refactor other crates except as required by moved module paths.

## Current State

- `connection.rs` is 1549 lines and mixes public entry points, socket I/O, request body handling, response encoding, telemetry guards, and tests.
- `runtime.rs` is 1807 lines and mixes public listener APIs, TCP/Unix accept loops, TLS accept loops, shutdown drain helpers, and feature-gated tests.
- `server.rs` is 865 lines and mixes the `NacelleServer` implementation with typed builder state.

## Ordered Work

1. Split `connection.rs` into parent entry points plus request/body/response/I/O/metrics/test modules.
2. Split `server.rs` into server methods and builder modules while re-exporting `NacelleServerBuilder` from `server`.
3. Split `runtime.rs` into public facade, TCP/Unix/plain listener, TLS listener, shared drain/accept utilities, and tests.
4. Update `.codex/skills/development/SKILL.md` with the repository file-size expectation.
5. Run formatting, clippy, and tests for the Rust workspace.

## Acceptance Gates

- Every `nacelle-tcp/src/**/*.rs` file is between 200 and 600 lines unless a small facade or model file is intentionally smaller.
- Existing public paths such as `nacelle_tcp::runtime::serve_tcp_*`, `nacelle_tcp::connection::serve_stream`, `TcpServer`, and `NacelleServerBuilder` continue to compile.
- `cargo fmt --all`, clippy with all features and targets, and workspace tests pass.

## Risks And Mitigations

- Feature-gated TLS symbols can break under `--all-features`; keep TLS helpers under the same `cfg` gates and validate with all features.
- Rust module privacy can break runtime/server internals; prefer `pub(crate)` or `pub(super)` only where existing internal call sites require it.
- Large mechanical moves can hide behavior changes; move code first, then only make minimal import and visibility fixes.