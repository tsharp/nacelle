# Nacelle Crate Split Plan

**Date:** 2026-06-06

**Objective:** Split the current `nacelle` crate into protocol-focused crates while keeping `nacelle` as the ergonomic umbrella crate. The reference protocol stays in `nacelle` proper, not in core.

## Target Crates

### `nacelle-core`

Owns transport-neutral application and runtime primitives:

- `Handler`, `HandlerFn`, `handler_fn`
- `NacelleRequest`, `NacelleResponse`, `NacelleBody`
- raw TCP and HTTP metadata structs needed by transports
- `NacelleError`
- `NacelleLimits`, `NacelleRuntimeState`, memory reservations, tracked permits
- lifecycle and shutdown types
- telemetry types and sinks
- Tokio spawn/join helper
- shared `NacelleTlsConfig` and optional self-signed generation
- optional Tower handler adapter

TLS remains here because it can serve both HTTP and raw TCP transport listeners.

### `nacelle-tcp`

Owns raw TCP transport behavior:

- `RawTcpServer`, `NacelleServer`, `NacelleServerBuilder`
- `Protocol`, `DecodedRequest`
- raw TCP connection serving functions
- Tokio TCP listener loops and graceful drain behavior

This crate should not contain the reference protocol implementation.

### `nacelle-http`

Owns HTTP transport behavior:

- `HyperServer`
- `NacelleHttpPolicy`
- HTTP/1 request/response adaptation
- HTTP listener loops and TLS-over-HTTP serving

This is the concrete home for the current HTTP edge-readiness work.

### `nacelle`

Remains the public convenience crate:

- depends on and re-exports `nacelle-core`, `nacelle-tcp`, and `nacelle-http` behind features
- owns the reference length-delimited protocol implementation
- preserves existing top-level exports such as `nacelle::RawTcpServer` and `nacelle::HyperServer`
- keeps examples, benches, fuzz tests, and docs anchored at the friendly entry point

## Feature Mapping

- `raw_tcp`: enables `nacelle-tcp`
- `reference_protocol`: enables `raw_tcp` and exposes the reference protocol from `nacelle`
- `http`: enables `nacelle-http` and core HTTP metadata types
- `tls`: enables core TLS and transport TLS integration
- `tls-self-signed`: enables core TLS plus self-signed certificate generation
- `otel`: enables core OpenTelemetry integration
- `tower`: enables core Tower handler adapter

## Execution Order

1. Add this plan and commit it.
2. Add workspace members and crate skeletons.
3. Extract `nacelle-core`, update the umbrella crate to depend on it, and preserve re-exports.
4. Extract `nacelle-tcp`, leaving the reference protocol in `nacelle`.
5. Extract `nacelle-http` and move HTTP policy/server code there.
6. Update stress tools, examples, benches, docs, and validation scripts.
7. Run the full production-readiness validation matrix and fix regressions.

## Compatibility Rules

- Preserve existing `nacelle::*` public imports wherever practical.
- Prefer feature-compatible re-exports over immediate breaking API renames.
- Keep TLS shared and transport-neutral.
- Do not move the reference protocol into `nacelle-core` or `nacelle-tcp`; it belongs to the umbrella `nacelle` crate.

