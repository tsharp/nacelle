# Serve API and Runtime Gap Plan

## Purpose

Make Nacelle easier to embed as an application runtime while preserving protocol
ownership outside the core crates. The target shape is a small public surface
where applications assemble an app, attach one or more protocol listeners, and
run them with `nacelle::serve(...).await`.

## Scope

- Add generic listener configuration for TCP nodelay, TCP keepalive, optional
  OpenSSL TLS detection on a shared TCP listener, and Unix socket lifecycle
  setup.
- Keep telemetry integration generic through `NacelleTelemetrySink`.
- Add an optional bridge from `tokio_util::sync::CancellationToken` to Nacelle
  shutdown.
- Add a higher-level serve/app builder that hides host/server wiring from
  protocol implementers without hiding the `Protocol` trait itself.
- Update public docs and examples for the new API.

## Non-Goals

- No downstream-specific names, protocol assumptions, metrics, or configuration
  schemas.
- No replacement for an application's request processor, auth, session, or data
  client code.
- No default stale Unix socket removal; lifecycle cleanup must be opt-in.
- No Rustls TLS sniffing in this pass unless the OpenSSL design reveals a shared
  abstraction that is low risk.
- No performance claims beyond local validation.

## Current State

- `NacelleServer` owns protocol, handler, limits, telemetry, and connection
  extension factories.
- `NacelleHost` owns multiple listeners and shared shutdown/drain state.
- Raw TCP, OpenSSL raw TCP TLS, Rustls raw TCP TLS, HTTP, and Unix sockets exist.
- TCP listeners set `TCP_NODELAY` but not keepalive.
- OpenSSL TLS currently requires a dedicated TLS listener.
- Unix socket paths are bound directly without cleanup or chmod.
- `NacelleTelemetrySink` is already the generic event bridge.
- Shutdown uses `NacelleShutdownToken`; no `CancellationToken` bridge exists.

## Phases

### 1. Plan Checkpoint

Write this plan and commit it before code changes.

Acceptance:
- Plan exists under `docs/internal`.
- Worktree is clean after commit.

Validation:
- `git status --short --branch`

### 2. Listener Options

Add stable transport option structs:
- TCP nodelay and keepalive settings.
- OpenSSL TLS detection timeout.
- Unix socket stale path cleanup and file permissions behind `cfg(unix)`.

Acceptance:
- Existing listener APIs keep their current behavior.
- New option APIs are opt-in and cloneable.
- Windows builds do not expose Unix-only APIs.

Validation:
- `cargo fmt --all -- --check`
- `cargo check --package nacelle-tcp --no-default-features`
- `cargo check --package nacelle --features reference_protocol`

### 3. Optional OpenSSL TLS Detection

Add a shared TCP listener mode that peeks at initial bytes and routes each
connection to plain raw protocol handling or OpenSSL TLS handling.

Acceptance:
- TLS-required OpenSSL listeners keep existing behavior.
- Optional TLS mode uses a bounded sniff timeout.
- Plain clients are not forced through TLS.
- TLS clients receive OpenSSL connection metadata.

Validation:
- Focused unit/integration checks for TLS detection helpers.
- Existing OpenSSL check where local OpenSSL can build.
- Existing raw TCP and TLS feature checks.

### 4. Unix Socket Lifecycle Helpers

Add opt-in Unix socket setup that can remove a stale path before bind and set
file permissions after bind.

Acceptance:
- Direct `serve_unix(path)` remains conservative.
- New configured path API documents its cleanup behavior.
- Pre-bound Unix listeners remain supported.

Validation:
- Windows compile proves Unix API gating.
- Unix CI should run an integration test for path cleanup and permissions.

### 5. Cancellation Bridge

Add an optional `tokio-util` feature that converts a `CancellationToken` into
Nacelle shutdown signaling without requiring every user to depend on
`tokio-util`.

Acceptance:
- Default dependency graph is unchanged.
- Bridge is small and explicit.
- Shutdown still drains through existing Nacelle deadlines.

Validation:
- `cargo check --package nacelle-core --features tokio-util`
- Targeted lifecycle test.

### 6. Serve/App API

Add a high-level app and protocol listener builder:
- The app holds handler, telemetry, limits, runtime state, and drain behavior.
- Protocol entries describe concrete raw protocol listeners.
- `nacelle::serve(app).await` runs the configured listeners.

Acceptance:
- Protocol implementers can keep protocol types separate from app state.
- Existing `RawTcpServer` and `NacelleHost` APIs continue to work.
- The new API can configure plain TCP, optional OpenSSL TLS TCP, and Unix
  sockets when features/platform permit.

Validation:
- Example or unit coverage for a simple raw protocol app.
- Feature checks across raw TCP, OpenSSL-gated APIs, HTTP/TLS combinations.

## Risks And Mitigations

- TLS sniffing can delay plain clients. Keep timeout explicit and low by default.
- Keepalive settings vary by OS. Use `socket2` narrowly and keep all options
  optional.
- Unix cleanup can remove the wrong file if misconfigured. Make cleanup opt-in,
  single-path only, and document ownership expectations.
- A high-level serve API could obscure useful lower-level control. Keep existing
  host/server/runtime APIs public and compose through them.
- OpenSSL vendored builds can fail on local toolchains. Treat local OpenSSL
  checks as best effort and rely on CI for provider coverage.

## Checkpoint Commits

1. `docs: plan serve API runtime gaps`
2. `feat: add listener runtime options`
3. `feat: add optional OpenSSL TCP detection`
4. `feat: add configured Unix socket binding`
5. `feat: add optional cancellation token bridge`
6. `feat: add high-level serve API`
7. `docs: document serve API and listener options`
