# Edge Readiness and Hardening Plan

**Date:** 2026-06-06

**Scope:** This plan defines what is needed for Nacelle to run as a direct public edge server without a reverse proxy or load balancer absorbing internet-facing risk first.

**Current status:** Nacelle is production-capable for controlled, internal, or proxy-protected deployments with explicit limits, shutdown, transport timeouts, telemetry, validation, and flat-memory soak behavior. The latest reported Linux soak ran for one hour at approximately 1.81M RPS, with stable memory, p99 latency around 3.5 ms, and clean shutdown. Direct public edge readiness is a separate higher bar.

**Implementation checkpoint:** Raw TCP and HTTP now both support shared Rustls TLS through `NacelleTlsConfig`; self-signed certificate generation is opt-in through `tls-self-signed`; certificate reloads update future handshakes; HTTP policy now includes security headers, structured access logs, and per-peer fixed-window request limits. TLS remains transport-neutral in `nacelle-core`, and `NacelleTlsProvider` keeps room for a future OpenSSL backend without changing listener APIs.

## Edge Readiness Definition

Nacelle should be considered direct-edge ready only when it can safely accept untrusted internet traffic by itself and provide:

- TLS termination with modern defaults and certificate rotation.
- Host/SNI validation and explicit listener policy.
- HTTP request parsing limits suitable for hostile clients.
- Per-client connection and request abuse controls.
- Predictable overload responses and low-cardinality telemetry.
- Request logging and operational runbooks for internet incidents.
- Sustained performance that preserves the existing internal hot path.
- CI and nightly validation for malformed, slow, high-churn, and long-running workloads.

Until these gates pass, the recommended public deployment remains:

```text
internet -> cloud load balancer / reverse proxy / service mesh -> Nacelle
```

## Architecture Strategy

Keep the core server path lean and make edge policy explicit.

- Add direct-edge behavior behind an `edge` feature or a companion `nacelle-edge` module/crate.
- Preserve the current raw/internal hot path when `edge` is disabled.
- Keep common state shared through `NacelleRuntimeState` so global limits, telemetry, and shutdown remain consistent.
- Prefer composable listener wrappers for TLS, peer identity, and policy rather than mixing every edge concern into handler dispatch.
- Benchmark edge-enabled and edge-disabled configurations separately.

## Phase 1: TLS Termination

**Objective:** Provide first-class TLS support with modern defaults and no hidden operational traps.

Deliverables:

- Add Rustls-based TLS listener support for HTTP.
- Support certificate/key loading from files.
- Support graceful certificate reload without process restart.
- Validate TLS config at startup and fail fast on invalid cert/key pairs.
- Support ALPN policy. HTTP/1 is required; HTTP/2 can remain explicitly unsupported until implemented.
- Expose TLS handshake errors and handshake timeout metrics.
- Document minimum TLS version, cipher defaults, certificate reload behavior, and operational examples.

Acceptance:

- TLS echo example works with generated local certificates.
- Invalid certificate config fails before binding or before accepting traffic.
- Certificate reload test proves new handshakes use the new certificate.
- TLS handshake timeout closes stalled clients.

## Phase 2: Host, SNI, and Listener Policy

**Objective:** Reject traffic that is not intended for the configured service.

Deliverables:

- Add optional allowed Host header list for HTTP.
- Add optional SNI allowlist for TLS listeners.
- Decide and document behavior when Host and SNI disagree.
- Add listener identity labels for logs and metrics.
- Emit rejection reason metrics without high-cardinality host labels.

Acceptance:

- Unknown Host requests receive a deterministic rejection.
- Unknown SNI handshakes are rejected or routed to a documented default.
- Host/SNI policy is configurable per listener.

## Phase 3: HTTP Edge Limits

**Objective:** Harden HTTP parsing and request lifecycle for hostile public clients.

Deliverables:

- Add explicit max request line length.
- Add explicit max URI length.
- Add explicit max header count.
- Add explicit max aggregate header bytes.
- Add optional method allowlist.
- Add request body rate/idle policy in addition to total body size.
- Add response write idle policy if current write timeout is not granular enough for slow readers.
- Return appropriate status codes where possible: `400`, `408`, `413`, `414`, `429`, `431`, or `503`.

Acceptance:

- Header bomb, long URI, slow header, slow body, and slow reader tests pass.
- Limit failures are observable as structured tracing events and metrics.
- Defaults are conservative for direct edge and documented for proxy-protected deployments.

## Phase 4: Per-Client Abuse Controls

**Objective:** Bound damage from a single remote source while preserving high aggregate throughput.

Deliverables:

- Track remote peer address at accept time.
- Add per-IP connection caps.
- Add optional per-IP request rate limits using a low-overhead token bucket or fixed-window design.
- Add accept-rate protection for connection churn.
- Add overload behavior that prefers fast rejection over queue growth.
- Add configuration to trust proxy-forwarded addresses only when explicitly enabled and only from trusted proxy CIDRs.

Acceptance:

- One source cannot consume all connection slots when per-IP caps are enabled.
- High-churn connection tests do not cause unbounded memory growth.
- Rate-limited requests produce deterministic status and metrics.
- Proxy address extraction is disabled by default for direct edge.

## Phase 5: Access Logs and Security Telemetry

**Objective:** Make public traffic diagnosable during normal operation and incidents.

Deliverables:

- Add structured access log events for HTTP requests.
- Include timestamp, listener, peer, method, path template or redacted path, status, latency, request bytes, response bytes, and request ID.
- Avoid logging full query strings or unbounded paths by default.
- Add rejection, timeout, TLS handshake, and overload counters.
- Provide dashboard guidance for active connections, RPS, rejection rates, p95/p99 latency, memory, and shutdown state.

Acceptance:

- Operators can distinguish normal 4xx traffic from overload and attack patterns.
- Metrics remain low-cardinality under hostile input.
- Access logging can be sampled or disabled for very high-throughput internal deployments.

## Phase 6: Security Headers and Optional CORS

**Objective:** Provide minimal HTTP response policy helpers without pretending to be an application firewall.

Deliverables:

- Add optional response header policy for common headers such as HSTS, X-Content-Type-Options, and X-Frame-Options.
- Add explicit CORS configuration helper or documentation for handler-level CORS.
- Keep defaults conservative and opt-in where application semantics matter.

Acceptance:

- Security headers can be enabled per HTTP listener.
- CORS policy is explicit and test-covered if implemented in the transport layer.

## Phase 7: Operational Deployment Pack

**Objective:** Give operators enough concrete material to run Nacelle at the edge.

Deliverables:

- Add direct-edge deployment guide.
- Add systemd example with file descriptor limits, restart policy, and graceful shutdown.
- Add Kubernetes example with readiness/liveness guidance and termination grace period.
- Add TLS certificate reload runbook.
- Add incident runbook for request flood, connection flood, slow clients, and certificate expiry.
- Add production config presets for internal, proxy-protected, and direct-edge modes.

Acceptance:

- A new operator can deploy a TLS-enabled edge example from the docs.
- Runbooks identify the metric symptoms and first mitigation steps.
- Config examples map directly to available code.

## Phase 8: Adversarial and Long-Run Validation

**Objective:** Prove the server behaves under public-internet failure modes.

Deliverables:

- Add malformed HTTP parser tests.
- Add slowloris and trickle-body regression tests.
- Add header bomb and URI-length tests.
- Add connection churn stress scenario.
- Add per-IP limit stress scenario.
- Add TLS handshake failure and timeout tests.
- Add nightly or manual 12 to 24 hour soak guidance for direct-edge settings.
- Add benchmark profiles for internal, proxy-protected, and direct-edge configurations.

Acceptance:

- Workspace validation passes.
- Feature matrix validation passes.
- `cargo audit` passes or advisories are explicitly accepted.
- Direct-edge soak shows stable memory and no rising active-connection/request gauges.
- Direct-edge throughput regression is understood and tracked against the internal baseline.

## Phase 9: Performance Guardrails

**Objective:** Add edge safety without silently giving up the raw throughput wins already recovered.

Deliverables:

- Add Criterion micro-benchmarks for hot limit-accounting paths.
- Add benchmarks for Host/SNI policy checks.
- Add benchmarks for per-IP accounting under representative concurrency.
- Add benchmark profiles with `edge` disabled and enabled.
- Document expected overhead for each edge feature.
- Keep high-cardinality maps, locks, and string allocation out of the default request path where possible.

Acceptance:

- Edge-disabled throughput remains near the current internal target.
- Edge-enabled overhead is measured and attributable.
- Any throughput drop is tied to a specific feature or policy decision.

## Release Gate

Direct public edge readiness requires all of the following:

- TLS listener support with reload tests.
- Host/SNI/listener policy tests.
- HTTP edge-limit tests.
- Per-client abuse-control tests.
- Access log and security telemetry docs.
- Deployment guide and incident runbooks.
- Full production validation script passes.
- `cargo audit` passes or accepted advisories are documented.
- At least one 12 hour direct-edge soak passes with flat memory.
- Benchmarks show no unexplained regression in edge-disabled mode.

## Suggested Implementation Order

1. Add edge config surface and docs skeleton.
2. Add TLS listener support and examples.
3. Add Host/SNI/listener policy.
4. Add HTTP parser/request limits.
5. Add per-IP connection and request controls.
6. Add access logs and security telemetry.
7. Add deployment pack and runbooks.
8. Add adversarial tests, long-run profiles, and benchmark guardrails.

This order gets certificate termination and request acceptance policy in place before adding finer abuse controls, while preserving a clear benchmark trail for each hardening step.
