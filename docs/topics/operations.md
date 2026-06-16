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

- `nacelle.connections.active`
- `nacelle.requests.active`
- `nacelle.streaming_tasks.active`
- `nacelle.memory.used_bytes`
- `nacelle.rejections`
- `nacelle.timeouts`
- `nacelle.request_errors`
- `nacelle.request_bytes`
- `nacelle.response_bytes`

Alerts should focus on sustained saturation, rising rejections, timeout spikes,
and memory approaching the configured budget.

## Benchmarking

The default TCP OpenTelemetry profile keeps lifecycle metrics on. Request
metrics are grouped under `NacelleTcpTelemetryConfig::request_metrics`:
`request_started`, `request_completed`, and `wire_byte_metrics` are on by
default, while `request_in_flight`, `request_duration_ms`, and phase histograms
are opt-in. The stress server's default OTel build prints a compact console
snapshot every 5 seconds.

Core request duration metrics are also opt-in through `NacelleTelemetryConfig`.
With the default config, core/HTTP request paths avoid request timer work unless
HTTP access logging is enabled.

Run microbenchmarks before and after hot-path changes:

```bash
cargo bench -p nacelle --features bench,reference_protocol
```


