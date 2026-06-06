# Nacelle Production Readiness Plan

> **For Hermes:** Use subagent-driven-development or focused TDD implementation passes to execute this plan task-by-task. Keep each change small, verified, and independently reviewable.

**Goal:** Make Nacelle battle-hardened for internet-facing and high-SLO production deployments.

**Architecture:** Build on the current hardening baseline: bounded defaults, shutdown primitives, accept-side raw TCP limits, tracked runtime counters, and passing workspace/feature-matrix validation. The remaining work should focus on adversarial transport behavior, richer observability, lifecycle draining/force-close semantics, dependency hygiene, and production documentation.

**Tech Stack:** Rust, Tokio, Hyper, http-body-util, tower optional integration, OpenTelemetry optional integration, Cargo test/clippy/fmt/audit, stress tooling under `nacelle-stress-*`.

---

## Current Context

Nacelle is now production-capable for controlled/internal deployments after the latest hardening pass. The following are already implemented and verified:

- `NacelleShutdown` / `NacelleShutdownToken` lifecycle primitive.
- Shutdown-aware HTTP and raw TCP listener paths.
- Host-level shutdown API.
- Raw TCP connection permits acquired before spawning per-connection tasks.
- Bounded default runtime limits.
- Tracked active connection/request/streaming/memory counters.
- HTTP request-body size and read-timeout enforcement.
- Clean validation across workspace tests, clippy, fmt, feature matrix, and cargo audit.

The codebase should not yet be considered fully battle-hardened for hostile internet-facing production because HTTP transport policy, telemetry exports, shutdown force-close semantics, and adversarial testing need deeper work.

---

## Production Readiness Definition

Nacelle should be considered production-ready when all of the following are true:

1. **Overload safety:** connection floods, request floods, slow clients, oversized bodies, and stalled consumers are rejected or timed out before unbounded memory/task growth.
2. **Lifecycle safety:** services can stop accepting, drain in-flight work, enforce a drain deadline, and force-close remaining work with observable outcomes.
3. **Observability:** production operators can answer how many connections/requests are active, how many were rejected/timed out, why failures happened, and which transport caused them.
4. **Transport hardening:** HTTP and raw TCP have explicit, documented timeout/keep-alive/body/header policies.
5. **Adversarial validation:** tests cover flood, trickle, cancellation, shutdown race, and protocol fuzz/property scenarios.
6. **Dependency hygiene:** vulnerability and deprecation scans are clean or explicitly accepted.
7. **Operator documentation:** configuration presets, deployment guidance, metrics, and shutdown behavior are documented.

---

## Phase 1: HTTP Transport Hardening

### Task 1.1: Add explicit HTTP timeout policy fields

**Objective:** Represent HTTP-specific transport deadlines and keep-alive behavior in config/limits instead of relying only on generic request/body timeouts.

**Files:**

- Modify: `nacelle/src/limits.rs`
- Modify: `nacelle/src/config.rs`
- Modify: `nacelle-stress-server/src/shared.rs` if stress config mirrors production limits
- Test: `nacelle/src/limits.rs`

**Implementation notes:**

Add fields such as:

```rust
pub http_header_read_timeout: Option<Duration>,
pub http_request_body_read_timeout: Option<Duration>,
pub http_response_write_timeout: Option<Duration>,
pub http_keep_alive_timeout: Option<Duration>,
pub http_max_connection_age: Option<Duration>,
```

If the existing `NacelleLimits` should stay transport-neutral, alternatively add a nested `NacelleHttpLimits` struct.

**Validation:**

```bash
cargo test -p nacelle limits::tests --all-targets
cargo clippy -p nacelle --all-targets -- -D warnings
```

---

### Task 1.2: Enforce HTTP header/read timeout around connection serving

**Objective:** Prevent slowloris-style clients from holding HTTP connections open before sending complete requests.

**Files:**

- Modify: `nacelle/src/http_server.rs`
- Test: `nacelle/src/http_server.rs`

**Approach:**

Wrap `http1::Builder::serve_connection(...)` or request-reading future with the new header/connection timeout where Hyper exposes the appropriate boundary. If Hyper does not expose a direct header-only future, document the limitation and enforce connection-level read/idle timeout around the full connection future.

**Test case:**

Create a TCP client that connects and sends only a partial HTTP request such as:

```text
GET / HTTP/1.1\r\nHost: localhost\r\n
```

Then wait beyond `http_header_read_timeout` and assert the connection is closed and active counters return to zero.

**Validation:**

```bash
cargo test -p nacelle http_slow_header_client_times_out --features http --all-targets
```

---

### Task 1.3: Enforce HTTP response write timeout / slow-reader protection

**Objective:** Prevent slow response consumers from pinning server tasks indefinitely.

**Files:**

- Modify: `nacelle/src/http_server.rs`
- Test: `nacelle/src/http_server.rs`

**Approach:**

Evaluate whether Hyper's body streaming can be wrapped with timeout semantics at the `HttpBodyStream::poll_next` boundary or whether write timeout must be enforced at connection level. Prefer connection-level enforcement if body polling does not map to socket write progress.

**Test case:**

Start a handler that produces a multi-chunk response. Connect a client that reads headers and then stops reading. Assert the server eventually times out/closes and counters drain.

**Validation:**

```bash
cargo test -p nacelle http_slow_response_reader_times_out --features http --all-targets
```

---

### Task 1.4: Document HTTP policy defaults and proxy assumptions

**Objective:** Make HTTP production behavior explicit for operators.

**Files:**

- Create or modify: `docs/http-hardening.md`
- Modify: `README.md`

**Document:**

- header timeout
- body timeout
- response write timeout
- keep-alive behavior
- max request/response body size
- whether deployment behind a proxy/load balancer is recommended
- how Nacelle behaves under slowloris/trickle clients

**Validation:**

```bash
cargo fmt --all -- --check
cargo test --workspace --all-targets
```

---

## Phase 2: Lifecycle Drain and Force-Close Semantics

### Task 2.1: Track spawned connection tasks explicitly

**Objective:** Allow shutdown to wait for active connection tasks and force-abort them after a deadline.

**Files:**

- Modify: `nacelle/src/runtime/tokio_rt/mod.rs`
- Modify: `nacelle/src/host.rs`
- Possibly create: `nacelle/src/lifecycle.rs` additions
- Test: `nacelle/src/server.rs` or `nacelle/src/host.rs`

**Approach:**

Introduce a connection task registry or per-listener `JoinSet` so accepted connections are not fire-and-forget. On shutdown:

1. stop accepting new connections;
2. wait for active connections to finish;
3. after drain timeout, abort remaining connection tasks;
4. record/log forced closures.

**Validation:**

```bash
cargo test -p nacelle raw_tcp_shutdown_aborts_after_drain_deadline --features reference_protocol --all-targets
```

---

### Task 2.2: Add staged shutdown events

**Objective:** Make shutdown behavior observable and diagnosable.

**Files:**

- Modify: `nacelle/src/telemetry.rs`
- Modify: `nacelle/src/host.rs`
- Modify: `nacelle/src/runtime/tokio_rt/mod.rs`

**Events/counters:**

- shutdown requested
- listener stopped accepting
- drain started
- drain completed
- drain timed out
- active connections aborted

**Validation:**

Add test telemetry sink or mock recorder and assert expected event sequence during shutdown.

---

### Task 2.3: Add shutdown race tests

**Objective:** Prove shutdown is safe while requests are in-flight, streaming, blocked, or erroring.

**Files:**

- Test: `nacelle/src/server.rs`
- Test: `nacelle/src/http_server.rs`
- Test: `nacelle/src/host.rs`

**Scenarios:**

- shutdown during raw TCP request body streaming
- shutdown during HTTP request body streaming
- shutdown while handler is sleeping
- shutdown while response body is still streaming
- repeated shutdown calls are idempotent

**Validation:**

```bash
cargo test -p nacelle shutdown --features reference_protocol,http --all-targets
```

---

## Phase 3: Production Observability

### Task 3.1: Add explicit rejection counters

**Objective:** Record why work was rejected instead of relying only on logs or generic errors.

**Files:**

- Modify: `nacelle/src/telemetry.rs`
- Modify: `nacelle/src/runtime/tokio_rt/mod.rs`
- Modify: `nacelle/src/connection.rs`
- Modify: `nacelle/src/http_server.rs`

**Counters:**

- connection rejected by limit
- request rejected by in-flight limit
- streaming task rejected by limit
- memory reservation rejected
- request body rejected by size
- response body rejected by size
- timeout by operation name

**Validation:**

Use a test telemetry sink to assert counters increment once per rejection.

---

### Task 3.2: Export active gauges through OpenTelemetry

**Objective:** Make active connection/request/streaming/memory counts visible in production dashboards.

**Files:**

- Modify: `nacelle/src/telemetry.rs`
- Modify: `nacelle/src/limits.rs`
- Test: `nacelle/src/telemetry.rs`

**Gauges:**

- `nacelle.connections.active`
- `nacelle.requests.active`
- `nacelle.streaming_tasks.active`
- `nacelle.memory.used_bytes`

**Validation:**

```bash
cargo test -p nacelle --features otel --all-targets
cargo clippy -p nacelle --features otel --all-targets -- -D warnings
```

---

### Task 3.3: Add per-transport byte metrics

**Objective:** Provide throughput and anomaly visibility.

**Files:**

- Modify: `nacelle/src/connection.rs`
- Modify: `nacelle/src/http_server.rs`
- Modify: `nacelle/src/telemetry.rs`

**Metrics:**

- request bytes read
- response bytes written
- request body chunks
- response body chunks
- protocol/error frame count

**Validation:**

Add unit/integration tests around known body sizes and assert telemetry values.

---

## Phase 4: Adversarial and Stress Testing

### Task 4.1: Add raw TCP connection-flood regression test

**Objective:** Prove connection saturation does not spawn unbounded tasks and counters remain bounded.

**Files:**

- Test: `nacelle/src/runtime/tokio_rt/mod.rs` or integration test under `nacelle/tests/`

**Scenario:**

Set `max_connections = 1`, hold one connection open, then open many additional connections. Assert:

- active connections never exceed 1;
- rejected connection metric increments;
- server remains responsive after the held connection closes.

**Validation:**

```bash
cargo test -p nacelle raw_tcp_connection_flood_is_bounded --features reference_protocol --all-targets
```

---

### Task 4.2: Add slow-client/trickle-body tests

**Objective:** Prove body-read timeout protects both HTTP and raw TCP from trickle clients.

**Files:**

- Test: `nacelle/src/server.rs`
- Test: `nacelle/src/http_server.rs`

**Scenarios:**

- raw TCP declares a body length but sends bytes too slowly;
- HTTP sends chunked/body data too slowly;
- both eventually produce timeout/error and drain counters.

**Validation:**

```bash
cargo test -p nacelle trickle --features reference_protocol,http --all-targets
```

---

### Task 4.3: Add protocol fuzz/property tests

**Objective:** Exercise frame parsing and response/error framing against random/malformed inputs.

**Files:**

- Modify: `nacelle/Cargo.toml`
- Create: `nacelle/tests/protocol_fuzz.rs` or use `proptest` inside `reference_protocol.rs`

**Approach:**

Use `proptest` to generate:

- random frame lengths;
- partial headers;
- bodies shorter/longer than declared;
- malformed flags/opcodes;
- fragmented valid frames.

**Validation:**

```bash
cargo test -p nacelle --features reference_protocol protocol_fuzz --all-targets
```

---

### Task 4.4: Convert stress tooling into CI-friendly scenarios

**Objective:** Ensure stress tests can run in bounded time in CI or nightly jobs.

**Files:**

- Modify: `nacelle-stress-test/src/main.rs`
- Modify: `nacelle-stress-server/src/shared.rs`
- Create: `docs/stress-testing.md`

**Scenarios:**

- baseline echo throughput
- max connection cap
- max request cap
- slow reader
- slow writer
- graceful shutdown under load

**Validation:**

Add documented commands with explicit duration, concurrency, and expected pass criteria.

---

## Phase 5: Dependency and Supply-Chain Hardening

### Task 5.1: Replace deprecated YAML stack in stress tooling/config

**Objective:** Remove dependency-quality concern around `serde_yaml` / `unsafe-libyaml` if feasible.

**Files:**

- Modify: `nacelle-stress-server/Cargo.toml`
- Modify: `nacelle-stress-server/src/shared.rs`
- Possibly modify config docs/examples

**Options:**

1. Replace YAML with TOML using `toml` crate.
2. Replace YAML with JSON using `serde_json`.
3. Keep YAML only if explicitly documented as non-production tooling and accepted.

**Validation:**

```bash
cargo test -p nacelle-stress-server --all-targets
cargo tree -i serde_yaml
cargo tree -i unsafe-libyaml
```

Expected after replacement: no dependency path unless intentionally retained.

---

### Task 5.2: Add advisory and dependency checks to CI

**Objective:** Make vulnerability scanning repeatable.

**Files:**

- Modify or create CI workflow files if present.
- Create: `docs/security-scanning.md`

**Commands:**

```bash
cargo audit
cargo deny check advisories bans licenses sources
```

If `cargo-deny` is adopted, add `deny.toml` with accepted licenses and source policy.

---

## Phase 6: Production Configuration and Operator Docs

### Task 6.1: Add production configuration guide

**Objective:** Explain how to configure Nacelle safely for real services.

**Files:**

- Create: `docs/production-configuration.md`
- Modify: `README.md`

**Include:**

- recommended defaults
- internal-service preset
- internet-facing-behind-proxy preset
- high-concurrency preset
- body size guidance
- timeout guidance
- memory budgeting formula
- examples of dangerous/unbounded configurations

---

### Task 6.2: Add production readiness checklist

**Objective:** Give deployers a final go/no-go checklist.

**Files:**

- Create: `docs/production-checklist.md`

**Checklist sections:**

- limits configured
- graceful shutdown wired
- metrics exported
- logs include rejection/timeout reasons
- load balancer/proxy behavior known
- stress tests run
- vulnerability scan clean
- rollback strategy documented

---

### Task 6.3: Document public API stability and semver policy

**Objective:** Prevent accidental breaking changes as the library matures.

**Files:**

- Create or modify: `docs/api-stability.md`
- Modify: `README.md`

**Include:**

- which APIs are stable
- which APIs are experimental
- feature flag policy
- semver behavior before/after `1.0`
- migration guidance for config/default changes

---

## Phase 7: CI / Release Gates

### Task 7.1: Add a complete validation script

**Objective:** Provide one command that runs the full production-readiness gate locally and in CI.

**Files:**

- Create: `scripts/validate-production-readiness.sh` or equivalent
- Document in: `docs/production-checklist.md`

**Script commands:**

```bash
cargo fmt --all -- --check
cargo test --workspace --all-targets
cargo clippy --workspace --all-targets -- -D warnings
cargo test -p nacelle --features reference_protocol,http,tower,otel --all-targets
cargo clippy -p nacelle --features reference_protocol,http,tower,otel --all-targets -- -D warnings
cargo test -p nacelle --no-default-features --features http --all-targets
cargo test -p nacelle --no-default-features --all-targets
cargo audit
```

**Validation:**

Run the script on a clean checkout and record expected output in the docs.

---

### Task 7.2: Add release-blocking CI jobs

**Objective:** Ensure the validation gate runs on PRs and release branches.

**Files:**

- Create/modify CI workflow files, depending on the repository's CI provider.

**Jobs:**

- fmt
- workspace tests
- workspace clippy
- feature matrix
- audit/deny
- optional stress smoke test

---

## Suggested Execution Order

1. Phase 1, Task 1.1: Add HTTP-specific limits.
2. Phase 1, Task 1.2: Slow header protection.
3. Phase 1, Task 1.3: Slow response writer protection.
4. Phase 2, Task 2.1: Connection task registry and force-close.
5. Phase 2, Task 2.2: Shutdown telemetry.
6. Phase 3, Task 3.1: Rejection counters.
7. Phase 3, Task 3.2: Active gauge export.
8. Phase 4, Tasks 4.1–4.3: Adversarial tests.
9. Phase 5: Dependency hardening.
10. Phase 6: Operator docs.
11. Phase 7: CI/release gates.

---

## Final Acceptance Gate

Before declaring Nacelle battle-hardened for internet-facing production, all commands below should pass on a clean checkout:

```bash
cargo fmt --all -- --check
cargo test --workspace --all-targets
cargo clippy --workspace --all-targets -- -D warnings
cargo test -p nacelle --features reference_protocol,http,tower,otel --all-targets
cargo clippy -p nacelle --features reference_protocol,http,tower,otel --all-targets -- -D warnings
cargo test -p nacelle --no-default-features --features http --all-targets
cargo test -p nacelle --no-default-features --all-targets
cargo audit
```

And these scenario classes should have automated coverage:

- HTTP slow header client.
- HTTP slow request body.
- HTTP slow response reader.
- raw TCP connection flood.
- raw TCP trickle body.
- shutdown during active streaming request.
- shutdown during active streaming response.
- drain deadline force-close.
- malformed frame fuzz/property tests.
- telemetry rejection/timeout counter assertions.

---

## Risks and Tradeoffs

- **Timeouts can break legitimate slow clients:** defaults should be conservative and documented, with explicit tuning guidance.
- **Force-closing connections can drop in-flight work:** staged shutdown should clearly differentiate graceful completion from forced abort.
- **More telemetry can add overhead:** counters/gauges should be low-cardinality and avoid per-request high-cardinality labels.
- **HTTP hardening may depend on Hyper internals:** prefer stable APIs and document any limitations.
- **Stress tests can be flaky in CI:** keep CI stress smoke tests short and deterministic; reserve heavy stress for nightly/manual runs.

---

## Open Questions

1. Should Nacelle be designed primarily for direct internet exposure, or assumed to run behind a reverse proxy/load balancer?
2. What are the target production envelopes: max connections, request rate, body sizes, and memory budget?
3. Should defaults prioritize safety over compatibility, even if they surprise benchmark users?
4. Should stress tooling be considered production-adjacent, or explicitly non-production/dev-only?
5. Which CI provider should own the release gate?
