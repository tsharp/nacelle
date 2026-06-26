# Operations model

## Deployment Shape

Recommended internet-facing shape:

```text
client -> proxy/load balancer/TLS -> Nacelle service
```

The proxy should own TLS, coarse connection filtering, and external idle
timeouts. Nacelle owns application limits, protocol handling, body limits, and
graceful shutdown.

## Startup

Use explicit limits and print the effective config for stress or benchmark
services. For production services, record:

- process version and git SHA
- configured limits
- listener addresses
- feature flags
- allocator settings

## Shutdown

Wire OS signals to `NacelleHost::shutdown_and_wait_timeout`. Pick a drain
deadline that matches service semantics. Short deadlines protect deploy velocity
but can abort in-flight work.

Expected shutdown telemetry:

- shutdown requested
- listener stopped accepting
- drain started
- drain completed or timed out
- active connections aborted

## Metrics To Watch

- `nacelle.runtime.connections.active`
- `nacelle.runtime.requests.active`
- `nacelle.runtime.streaming_tasks.active`
- `nacelle.runtime.memory.used_bytes`
- `nacelle.connections.accepted`
- `nacelle.connections.closed`
- `nacelle.requests.started`
- `nacelle.requests.completed`
- `nacelle.rejections`
- `nacelle.timeouts`
- `nacelle.requests.failed`
- `nacelle.request.bytes`
- `nacelle.response.bytes`

Alerts should focus on sustained saturation, rising rejections, timeout spikes,
and memory approaching the configured budget.

## Benchmarking

The default OpenTelemetry profile keeps lifecycle metrics on. Request metrics
are grouped under `NacelleTelemetryConfig::request_metrics`: `started`,
`completed`, and `byte_counts` are on by default, while `in_flight`,
`duration_ms`, and phase histograms are opt-in. The stress server's default OTel
build prints a compact console snapshot every 5 seconds.

Request duration metrics remain opt-in through `NacelleTelemetryConfig`. With
the default config, core/HTTP request paths avoid request timer work unless HTTP
access logging is enabled.

Run microbenchmarks before and after hot-path changes:

```bash
cargo bench -p nacelle --features bench,reference_protocol
```

