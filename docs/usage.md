# Usage Guide

Nacelle exposes one handler shape across raw TCP and HTTP:

```rust
async fn handle(request: NacelleRequest) -> Result<NacelleResponse, NacelleError>
```

Use `handler_fn` for simple services and capture application state in an `Arc`.

The top-level `nacelle` crate re-exports the split transport crates. Use
`nacelle-core` directly for shared primitives, `nacelle-tcp` for raw TCP-only
applications, and `nacelle-http` for HTTP-only applications when you want a
narrow dependency surface.

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

Enable `tls` to terminate raw TCP over Rustls with the same shared TLS config
used by HTTP:

```rust
let tls = NacelleTlsConfig::from_pem_files("cert.pem", "key.pem")?;
server.serve_tcp_tls("127.0.0.1:8443".parse()?, tls).await?;
```

For direct edge listeners, prefer an SNI allowlist:

```rust
let cert = std::fs::read("cert.pem")?;
let key = std::fs::read("key.pem")?;
let tls = NacelleTlsConfig::from_pem_with_allowed_server_names(
    &cert,
    &key,
    ["api.example.com"],
)?;
```

## HTTP

The HTTP transport is HTTP/1 through Hyper.

```rust
let server = HyperServer::new(handler_fn(|request: NacelleRequest| async move {
    Ok(NacelleResponse::http_bytes(http::StatusCode::OK, "ok"))
}));

server.serve("127.0.0.1:8081".parse()?).await?;
```

Enable the `tls` feature to terminate HTTPS directly through Rustls:

```rust
let tls = NacelleTlsConfig::from_pem_files("cert.pem", "key.pem")?;
server.serve_tls("127.0.0.1:8443".parse()?, tls).await?;
```

`NacelleTlsConfig` is transport-neutral and does not inject HTTP-specific ALPN
into raw TCP listeners. Existing listeners pick up certificate/key reloads for
new handshakes:

```rust
let tls = NacelleTlsConfig::from_pem_files("cert.pem", "key.pem")?;
tls.reload_from_pem_files("next-cert.pem", "next-key.pem")?;
```

Enable `tls-self-signed` for local load tests or auto-deploy flows that need an
immediate self-signed certificate:

```rust
let generated = NacelleTlsConfig::self_signed(["localhost", "127.0.0.1"])?;
server.serve_tls(addr, generated.tls_config).await?;
```

For HTTP edge policy, attach `NacelleHttpPolicy` to reject unwanted requests
before the handler runs:

```rust
let policy = NacelleHttpPolicy::new()
    .with_allowed_hosts(["api.example.com"])
    .with_allowed_methods([http::Method::GET, http::Method::POST])
    .with_max_uri_len(4096)
    .with_max_header_count(64)
    .with_max_header_bytes(16 * 1024)
    .with_max_requests_per_peer_per_second(1_000)
    .with_default_security_headers();

let server = HyperServer::new(handler)
    .with_http_policy(policy)
    .with_access_log(true);
```

HTTP deployments may still sit behind a proxy or load balancer for coarse
traffic filtering. Nacelle enforces request, body, timeout, TLS handshake,
connection, optional per-peer connection/request caps, optional security
headers, and optional Host/header/method/URI policy in-process.

## Multi-Listener Host

Use `NacelleHost` when one process owns several listeners and should share one
limit budget.

```rust
let limits = NacelleLimits::default().with_max_connections(16_384);
let limits = limits.with_max_connections_per_peer(512);
let limits = limits.with_max_connection_opens_per_peer_per_second(128);
let mut host = NacelleHost::new().with_limits(limits);
host.enable_raw_tcp("raw", raw_server_addr, raw_server)
    .enable_http("http", http_addr, http_server);

host.wait().await?;
```

With the `tls` feature enabled, use `enable_http_tls` for a host-managed HTTPS
listener, or `enable_raw_tcp_tls` for a host-managed raw TCP TLS listener:

```rust
host.enable_http_tls("https", https_addr, http_server, tls_config);
host.enable_raw_tcp_tls("raw-tls", raw_tls_addr, raw_server, raw_tls_config);
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
- optional per-peer connections
- optional per-peer connection-open rates
- memory reservations
- request and response body size
- raw and HTTP timeouts
- TLS handshake timeout when `tls` is enabled

Use explicit limits in production. Avoid `usize::MAX` outside benchmarks.

## Telemetry

`NacelleTelemetry` emits tracing events by default and OpenTelemetry metrics when
the `otel` feature is enabled. Attach an in-memory sink in tests when you need to
assert rejection, timeout, shutdown, or byte-accounting events.

When `HyperServer::with_access_log(true)` is enabled, HTTP requests also emit
structured `nacelle::access` tracing events with transport, method, URI, status,
request bytes, elapsed microseconds, and low-cardinality rejection reason.
