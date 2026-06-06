# Nacelle Production Readiness Assessment

Date: 2026-06-05

## Determination

Nacelle is a solid high-performance prototype, not production-ready yet.

The core design is promising: concrete handler types, low allocation pressure, optional global limits, streaming body shape, feature-gated transports, and a clean handler API. It is suitable for controlled benchmarks, internal experiments, and production-shape prototyping.

It should not be treated as a battle-hardened production service yet, especially at 1,500 to 128,000 max connections, until lifecycle, memory accounting, backpressure, and observability gaps are addressed.

## Current Strengths

- Concrete handler types avoid boxed per-request dispatch in the common path.
- Raw TCP and HTTP share the same application-facing `NacelleRequest` / `NacelleResponse` shape.
- Runtime limits exist for connections, in-flight requests, streaming tasks, request bytes, response bytes, memory bytes, and timeouts.
- Limits can be shared across listeners through `NacelleRuntimeState` / `NacelleHost`.
- The reference protocol is optional and not enabled by default.
- The stress harness now prints effective config and server-side runtime stats.
- The current workspace and feature matrix pass tests and clippy.

## Findings

### High: No Production Lifecycle Or Shutdown Model

Raw TCP and HTTP listeners run infinite accept loops with no shutdown token or graceful drain path. `NacelleHost::wait()` waits for listener tasks to end, which normally happens only on error.

Production needs:

- stop accepting new connections
- drain active requests
- deadline-based graceful shutdown
- force-close after timeout
- shutdown/drain telemetry

### High: Memory Budget Is Incomplete

Raw TCP reserves initial per-connection buffers and buffered request bodies, but the memory budget does not fully cover:

- streaming request chunks
- response encoding buffer growth
- protocol/frame overhead
- HTTP body queueing
- Hyper internal buffers
- application allocations
- backend/client pools

For 128k connections and responses up to 15MB, this is still an OOM risk unless the process/container has external guardrails and conservative config.

### High: Defaults Are Prototype/Performance Defaults

The default limits are effectively unlimited for:

- max connections
- max in-flight requests
- max streaming tasks
- max memory bytes

Raw TCP also defaults to buffered request bodies and 64KiB read/response buffers. That is acceptable for benchmarking, but production should require explicit sizing or provide SKU-safe presets.

### Medium: Connection Limiting Happens After Accept/Spawn

Raw TCP accepts a connection and spawns a task before the connection permit is acquired inside `serve_io`. Under connection floods, the process can churn accepting, spawning, and closing rather than applying accept-side backpressure.

Production needs:

- accept-side connection permits
- configurable listen backlog
- accept error telemetry
- overload behavior that is explicit and measured

### Medium: HTTP Transport Is Not Production-Grade Yet

The HTTP transport is useful but still minimal:

- HTTP/1 only
- no TLS integration
- no graceful shutdown
- no configurable keep-alive/header/read timeouts
- no request byte metrics
- no response byte metrics
- limited memory accounting for body queues

HTTP telemetry currently records request/response byte counts as zero.

### Medium: Observability Is Useful But Incomplete

Nacelle emits tracing events and optional OTel counters, but production operation needs more runtime signals:

- active connection gauge
- active request gauge
- limit rejection counters by reason
- memory-used gauge
- accept error counter
- queue depth / body channel pressure
- shutdown and drain events
- per-listener labels
- transport-specific error classes

### Medium: Raw TCP Pipelining Semantics Need To Be Explicit

Raw TCP currently processes requests sequentially per connection. Client pipelining can queue pressure, but it does not mean concurrent handler execution per connection.

That may be the right tradeoff for ordering and throughput, but it should be documented and tested as a protocol guarantee.

### Low/Medium: Production Test Coverage Is Thin

The current tests are good for the prototype surface, but production readiness needs broader coverage:

- malformed stream fuzzing
- slowloris behavior
- timeout races
- cancellation behavior
- limit rejection under load
- shutdown drain behavior
- HTTP limit enforcement
- memory budget enforcement
- high-connection soak tests
- overload tests

## Quality Assessment

Code quality is good for this stage.

The core is small, readable, and avoids accidental abstraction. The performance instincts are sound: concrete handlers, bounded channels, optional semaphores, and no obvious hot-path `Box<dyn>` issue.

No Rust-level safety problems were identified in this review.

The biggest risks are not code style problems. They are production systems concerns:

- lifecycle
- memory completeness
- backpressure
- overload behavior
- observability depth
- operational testing

## Readiness Call

| Use case | Readiness |
| --- | --- |
| Prototype | Yes |
| Benchmark harness | Yes |
| Internal controlled service | Maybe, with explicit limits and process supervision |
| Internet-facing production service | No |
| Critical production service | No |
| 128k connection SKU | No, not until memory/backpressure/shutdown are hardened |

## Recommended Next Steps

1. Add graceful shutdown and drain support across `NacelleHost`, raw TCP, and HTTP.
2. Move connection permits to the accept side and make listen backlog configurable.
3. Tighten memory accounting for streaming queues, response buffers, HTTP bodies, and queued chunks.
4. Add production-safe presets or require explicit limits for production mode.
5. Add active gauges and rejection metrics for OTel/tracing.
6. Harden HTTP configuration: timeouts, keep-alive behavior, byte metrics, graceful shutdown.
7. Document raw TCP per-connection sequential processing and pipelining semantics.
8. Add fuzz, overload, slow-client, cancellation, and shutdown-drain tests.

## Verification Run During Assessment

The following checks passed during this assessment:

```text
cargo test --workspace --all-targets
cargo clippy --workspace --all-targets -- -D warnings
cargo test -p nacelle --features reference_protocol,http,tower,otel --all-targets
cargo clippy -p nacelle --features reference_protocol,http,tower,otel --all-targets -- -D warnings
cargo test -p nacelle --no-default-features --features http --all-targets
cargo test -p nacelle --no-default-features --all-targets
```
