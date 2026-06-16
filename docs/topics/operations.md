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

The default TCP OpenTelemetry profile keeps lifecycle/request metrics on and
leaves active request gauges, phase histograms, and opcode labels off. The stress
server's default OTel build turns on TCP request/response byte counters and
prints a compact console snapshot every 5 seconds. Turn the remaining detailed
TCP metrics on only for diagnostic runs; they add per-request writes and
attribute work.

Run microbenchmarks before and after hot-path changes:

```bash
cargo bench -p nacelle --features bench,reference_protocol
```


