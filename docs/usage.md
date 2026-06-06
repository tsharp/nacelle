# Usage Guide

Nacelle exposes one handler shape across raw TCP and HTTP:

```rust
async fn handle(request: NacelleRequest) -> Result<NacelleResponse, NacelleError>
```

Use `handler_fn` for simple services and capture application state in an `Arc`.

## Raw TCP

Raw TCP requires a protocol implementation. The built-in
`LengthDelimitedProtocol` is an optional reference protocol enabled by
`reference_protocol`.

```rust
let server = RawTcpServer::<FrameRequest, ()>::builder()
    .protocol(LengthDelimitedProtocol)
    .handler(handler_fn(|request: NacelleRequest| async move {
        Ok(NacelleResponse::raw_tcp(request.body))
    }))
    .build()?;

server.serve_tcp("127.0.0.1:8080".parse()?).await?;
```

Raw TCP processes requests sequentially per connection. This preserves response
ordering and keeps per-connection concurrency predictable. Use multiple
connections when clients need parallelism.

## HTTP

The HTTP transport is HTTP/1 through Hyper.

```rust
let server = HyperServer::new(handler_fn(|request: NacelleRequest| async move {
    Ok(NacelleResponse::http_bytes(http::StatusCode::OK, "ok"))
}));

server.serve("127.0.0.1:8081".parse()?).await?;
```

HTTP deployments should generally sit behind a TLS-terminating proxy or load
balancer. Nacelle still enforces request, body, timeout, and connection limits.

## Multi-Listener Host

Use `NacelleHost` when one process owns several listeners and should share one
limit budget.

```rust
let limits = NacelleLimits::default().with_max_connections(16_384);
let mut host = NacelleHost::new().with_limits(limits);
host.enable_raw_tcp("raw", raw_server_addr, raw_server)
    .enable_http("http", http_addr, http_server);

host.wait().await?;
```

For graceful shutdown, call `shutdown_and_wait_timeout`. Listeners stop
accepting, connection tasks drain, and remaining work is aborted after the
deadline.

```rust
host.shutdown_and_wait_timeout(Duration::from_secs(30)).await?;
```

## Limits

`NacelleLimits` controls process-wide runtime budgets:

- active connections
- in-flight requests
- streaming body tasks
- memory reservations
- request and response body size
- raw and HTTP timeouts

Use explicit limits in production. Avoid `usize::MAX` outside benchmarks.

## Telemetry

`NacelleTelemetry` emits tracing events by default and OpenTelemetry metrics when
the `otel` feature is enabled. Attach an in-memory sink in tests when you need to
assert rejection, timeout, shutdown, or byte-accounting events.
