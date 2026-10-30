# Performance model

nacelle's high-throughput TCP path is sensitive to small per-request costs.
When comparing runs, keep these variables fixed:

- commit
- Linux kernel and CPU governor
- allocator configuration
- server threads
- connection count
- pipeline depth
- payload size
- TLS versus plain TCP
- stress client version

Use the performance how-to for repeatable command lines.

Do not use a repository-wide RPS number as a baseline. Compare commits on the
same host with the same kernel, governor, allocator, features, transport/TLS
mode, configuration, workload, and client revision.

Suggested local benchmark:

```bash
cargo bench -p nacelle-examples --features "bench tcp"
```

The `runtime_limits` benchmark group covers connection/request permit
acquire/drop overhead. Watch it closely after changes to `NacelleRuntimeState`.

## Successful-path ownership and dispatch

### Connection tracking costs

Tracking is disabled by default. Enabling it adds connection-time registry
allocation/locking and a cancellation waiter per connection. TCP uses atomic
activity transitions at input/idle boundaries. HTTP also waits for graceful
retirement but adds no request activity hooks; it cannot prove safe idle
eligibility. Only a selected retirement creates an administrative timer.
Tracking does not add per-request registry locks,
timers, or metrics. Bounded registry map storage retains its high-water
allocation; TLS metadata, snapshots retained by callers, sockets, and transport
buffers must be budgeted separately.

Run the explicit lifecycle profiles:

```bash
cargo bench -p nacelle-examples --features bench,tcp --bench connection_lifecycle
```

`registry_churn` compares registration/drop with tracking off/on.
`tracking_tcp` compares one and 64 requests per direct-I/O connection, with
immediate delivery and metrics disabled. These are in-memory driver cases,
not network throughput. The 256-entry idle/reclaim profiles isolate registry
administration; the four-worker profile also includes channel dispatch/replies,
but excludes worker thread startup. Registry-only entries contain no TCP tasks.

On Linux, the same benchmark binary accepts
`NACELLE_TRACKING_MEMORY_ENTRIES=100000`. Run that mode in a fresh process.
It touches guard storage before observing RSS, registers plain metadata without
peer/TLS strings, then observes live and retired RSS. The difference estimates
registry/signal/allocator bytes per live entry, not total connection memory or
exact allocated bytes. Page rounding and allocator retention affect the result.
Keep allocator, entry count, toolchain, and platform fixed between comparisons.

Compare tracking-off network throughput with the retained baseline and frozen
stress client as well; the absence of registry operations does not prove that
future layout or code generation has no cost.

For HTTP, keep the unchanged `critical_paths` HTTP round-trip cases as the
tracking-off baseline and run the separate opt-in profiles:

```bash
cargo bench -p nacelle-examples --features bench,http --bench http_connection_lifecycle
NACELLE_HTTP_TRACKING=0 NACELLE_HTTP_MEMORY_CONNECTIONS=2048 cargo bench -p nacelle-examples --features bench,http --bench http_connection_lifecycle
NACELLE_HTTP_TRACKING=1 NACELLE_HTTP_MEMORY_CONNECTIONS=2048 cargo bench -p nacelle-examples --features bench,http --bench http_connection_lifecycle
NACELLE_HTTP_TRACKING=0 NACELLE_HTTP_LATENCY_REQUESTS=50000 cargo bench -p nacelle-examples --features bench,http --bench http_connection_lifecycle
NACELLE_HTTP_TRACKING=1 NACELLE_HTTP_LATENCY_REQUESTS=50000 cargo bench -p nacelle-examples --features bench,http --bench http_connection_lifecycle
```

`http_tracking` compares one and 64 sequential requests per loopback connection
using identical 32-byte fixed-length responses, a two-worker Tokio runtime, and
disabled metrics. It includes client parsing/allocation, network I/O, and
connection churn, not just registry work. `http_retirement` measures selection
through client-observed EOF and capacity release for established keep-alive
connections; setup and its warm-up request are excluded. Pending production,
flush, and deadline cancellation are deterministic tests, not latency claims.

The Linux memory mode holds warmed client/server pairs in one process. Its RSS
delta includes both endpoints, runtime tasks, buffers, registry, and allocator
retention, not isolated server allocations or kernel socket memory. Run each
mode in a fresh process with the same live count. The latency mode reports
client-observed p50/p99 on one warmed persistent connection; these distributions
are separate from Criterion batch means. Alternate tracking-off/on repetitions
without concurrent builds, keep CPU affinity and feature flags fixed, and do
not generalize these plain HTTP profiles to HTTPS or worker-local topology.

### Request ownership

The default successful TCP and HTTP pipelines use concrete protocol, handler,
responder, body, and telemetry observer types. Nacelle does not box handler
futures or dynamically dispatch those contracts.

Retained costs are scoped by ownership:

- TCP connection buffers and decoder state are created per connection and reused
	across requests. Oversized response frames and optional input-buffer rotation
	can replace those buffers deliberately.
- A non-empty HTTP request uses one bounded Tokio channel for the request-body
	bridge; an exact zero-length body skips the channel and producer work, while
	the small scoped future still completes immediately.
- HTTP response bodies remain concrete `StreamBody` values rather than boxed
	body trait objects.
- Request/handler/read/write timeouts use concrete futures. The HTTP response
	write deadline retains a boxed `Sleep` only after connection-level
	backpressure because Hyper requires its I/O wrapper to remain `Unpin`.
- Enabled per-peer request and connection-open rate limits use fixed-capacity,
  lock-free tables. Admission probes a bounded number of atomic slots and
  rejects a newly observed peer when the configured table is full; it does not
  take a per-request mutex or sweep every tracked peer.
- Type-erased protocol/handler errors are constructed only on error paths.
- TCP response coalescing is opt-in. It queues only complete bounded frames and
	restores socket backpressure at thresholds, streaming waits, and socket-read
	boundaries.
- App listener installers and worker thread closures erase startup-only closure
	types; they are not involved in request dispatch.
- Optional tracing, Hyper, Tokio, TLS providers, allocators, and metrics
	recorders retain their own external indirection.

TCP computes effective telemetry modes once per connection and constructs one
`NacelleMetricsContext` containing cached facade handles. Without an installed
recorder, metric writes through those handles are no-ops. Connection/request
permits update cached runtime gauges at their existing state transitions.
Diagnostic TCP phase timers are compiled only with
the non-default `phase-timing` feature and remain runtime-disabled until
explicitly enabled on `NacelleTelemetry`.

Enable the `buffer-rotation` feature for long-lived TCP connections that may
occasionally receive large requests. Once an oversized cumulative input buffer
is empty, Nacelle replaces it with a buffer sized to `read_buffer_capacity`.
Leave the feature disabled when retaining peak buffer capacity is preferable to
allocating again after traffic spikes.

Suggested RPS comparison:

```bash
./examples/run-stress-test.sh --config examples/nacelle-stress-server/configs/tcp.toml --server-threads 48 --connections 256 --pipeline 8 --duration-secs 30 --payload-bytes 256
```

The stress server installs a `metrics-util` debugging recorder and prints a
compact console snapshot every 5 seconds. Request metrics are grouped under the generic telemetry
`request_metrics` config; started/completed counters and byte counters are on by
default, while in-flight and duration metrics remain opt-in. Request duration
metrics are opt-in as well, which avoids request `Instant` work on core/HTTP
paths unless duration metrics or HTTP access logs are enabled. Use
`--no-byte-metrics` when comparing the cost of byte accounting. Use the
`telemetry_paths` benchmark for a no-recorder facade baseline; the stress server
intentionally installs its console recorder in every feature set.

The checked-in root `config.toml` enables self-signed TCP TLS for local
stress runs. For the plain TCP throughput baseline, use
`examples/nacelle-stress-server/configs/tcp.toml`. Compare TLS and non-TLS runs
separately:

```bash
./examples/run-stress-test.sh --config examples/nacelle-stress-server/configs/tcp.toml
./examples/run-stress-test.sh --config examples/nacelle-stress-server/configs/tcp-tls.toml
```

The `examples/run-stress-test.sh` and `examples/run-stress-test.ps1` helpers
apply root `config.toml` first, then the selected profile, and choose the
matching client mode automatically.

Guardrails:

- keep shutdown task tracking at the connection/listener boundary
- avoid per-request locks in the TCP hot path
- keep telemetry observers optional; default operation uses `NoopObserver`
- preserve single-chunk body fast paths
- tune TCP buffer sizes for the connection count instead of relying on large defaults
