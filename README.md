# Nacelle

**Nacelle is an experimental Tokio-based Rust library for streaming application handlers across multiple transports.**

The current transports are:

- `tcp` (default) — custom protocol transport over TCP and Unix sockets
- `reference_protocol` — optional length-delimited example protocol
- `http` — Hyper HTTP/1 server transport
- `tls` — provider-neutral TLS capability shared by TLS backends
- `rustls` — Rustls-backed TLS termination for HTTP and TCP
- `openssl` — OpenSSL-backed TLS termination for TCP
- `openssl-vendored` — build OpenSSL from source when native OpenSSL is unavailable
- `tls-self-signed` — optional Rustls self-signed TLS generation for local load tests and auto-deploy flows
- `otel` — OpenTelemetry metrics API integration
- `tokio-util` — bridge `tokio_util::sync::CancellationToken` into Nacelle shutdown
- `tower` — adapter for `tower::Service<NacelleRequest>`

Both transports call the same app-facing handler shape:

```rust
async fn handle(request: NacelleRequest) -> Result<NacelleResponse, NacelleError>
```

`NacelleBody` is streaming, so handlers can consume request chunks and return response chunks without forcing full buffering.

## Crate Layout

The workspace is split by responsibility:

- `nacelle-core` owns shared handler, body, limits, telemetry, lifecycle, and TLS primitives.
- `nacelle-tcp` owns the TCP transport and protocol trait.
- `nacelle-http` owns the Hyper HTTP/1 transport and HTTP edge policy.
- `nacelle` is the convenience crate that re-exports the transport crates and owns the reference length-delimited protocol.

TLS is shared core infrastructure. `tls` is provider-neutral. `rustls` enables
the Rustls provider for HTTP and TCP. `openssl` enables the TCP OpenSSL
provider through `NacelleOpenSslConfig` and `serve_tcp_openssl` without enabling
or consuming Rustls.

## TCP Example

```rust
use nacelle::{
    FrameRequest, LengthDelimitedProtocol, NacelleError, NacelleRequest, NacelleResponse,
    TcpServer, handler_fn,
};

let server = TcpServer::<FrameRequest, ()>::builder()
    .protocol(LengthDelimitedProtocol)
    .handler(handler_fn(|mut request: NacelleRequest| async move {
        let opcode = request.tcp_opcode().unwrap_or_default();
        if opcode != 1 {
            while let Some(chunk) = request.body.next_chunk().await {
                let _ = chunk?;
            }
            return Err(NacelleError::handler(std::io::Error::other(
                format!("unknown opcode {opcode}"),
            )));
        }
        Ok(NacelleResponse::tcp(request.body))
    }))
    .build()?;

server.serve_tcp("127.0.0.1:8080".parse()?).await?;
```

For applications with one handler and one or more protocol listeners, the
convenience crate also exposes an app-level serve API:

```rust
use nacelle::{
    FrameRequest, LengthDelimitedProtocol, NacelleApp, NacelleProtocols, NacelleRequest,
    NacelleResponse, handler_fn, serve,
};

let app = NacelleApp::new(handler_fn(|request: NacelleRequest| async move {
    Ok(NacelleResponse::tcp(request.body))
}));

let protocols = NacelleProtocols::new().tcp::<FrameRequest, _>(
    "tcp",
    "127.0.0.1:8080".parse()?,
    LengthDelimitedProtocol,
);

serve(protocols, app).await?;
```

On Unix, the same protocol can listen on a Unix domain socket:

```rust
server.serve_unix("/tmp/nacelle.sock").await?;
```

By default, Nacelle does not remove existing socket files before binding. Use
`NacelleUnixSocketOptions` when the process owns the path and should remove a
stale file or set socket-file permissions.

TCP listeners accept `NacelleTcpOptions` for `TCP_NODELAY` and keepalive. With
the `openssl` feature, `serve_tcp_optional_openssl(...)` and
`NacelleProtocols::tcp_optional_openssl(...)` can accept plain and OpenSSL
TLS clients on the same TCP listener after a bounded TLS-prefix peek.

## Shared Application State

Nacelle does not bake in service registries. Capture your app state in the concrete handler:

```rust
use std::sync::Arc;

struct AppState {
    data_client: DataClient,
    session_manager: SessionManager,
}

let app = Arc::new(AppState {
    data_client,
    session_manager,
});

let handler = handler_fn({
    let app = app.clone();
    move |request: NacelleRequest| {
        let app = app.clone();
        async move {
            // app.data_client uses one shared pool internally.
            Ok(NacelleResponse::empty_tcp())
        }
    }
});
```

## Connection Metadata

Every handler receives `request.connection`, which includes transport, peer
address, local address, local Unix socket path, effective peer IP, and TLS metadata when available. Raw
TCP servers can attach a typed per-connection extension at accept time:

```rust
#[derive(Clone)]
struct ConnectionContext {
    peer: Option<std::net::SocketAddr>,
}

let server = TcpServer::<FrameRequest, ()>::builder()
    .protocol(LengthDelimitedProtocol)
    .connection_extension_factory(|connection| ConnectionContext {
        peer: connection.peer_addr,
    })
    .handler(handler_fn(|request: NacelleRequest| async move {
        let _context = request.connection.extension::<ConnectionContext>();
        Ok(NacelleResponse::empty_tcp())
    }))
    .build()?;
```

## Multi-Port Host

`NacelleHost` centralizes listener setup while each server keeps its concrete protocol and handler types:

```rust
let telemetry = NacelleTelemetry::default();
let raw_docs = TcpServer::<FrameRequest, ()>::builder()
    .protocol(LengthDelimitedProtocol)
    .telemetry(telemetry.clone())
    .handler(handler.clone())
    .build()?;
let http_api = HyperServer::new(handler).with_telemetry(telemetry.clone());

let mut host = NacelleHost::new().with_telemetry(telemetry);
host.enable_tcp("docs-reference", "127.0.0.1:8080".parse()?, raw_docs)
    .enable_http("http-api", "127.0.0.1:8081".parse()?, http_api);
host.wait().await?;
```

Custom protocols use the same `TcpServer::<YourRequest, ()>::builder().protocol(your_protocol)` path, so multiple TCP protocols can listen on different ports.

## Production Limits

Use one shared `NacelleRuntimeState` or `NacelleHost::with_limits(...)` to enforce global budgets across all listeners:

```rust
use std::time::Duration;

let limits = NacelleLimits::default()
    .with_max_connections(128_000)
    .with_max_in_flight_requests(64_000)
    .with_max_streaming_tasks(8_192)
    .with_max_memory_bytes(8 * 1024 * 1024 * 1024)
    .with_max_request_body_bytes(16 * 1024 * 1024)
    .with_max_response_body_bytes(16 * 1024 * 1024)
    .with_read_timeout(Duration::from_secs(30))
    .with_write_timeout(Duration::from_secs(30))
    .with_handler_timeout(Duration::from_secs(60))
    .with_idle_timeout(Duration::from_secs(120));

let mut host = NacelleHost::new()
    .with_telemetry(telemetry)
    .with_limits(limits);
```

For very high connection counts, size buffers deliberately. The default TCP read and response buffers are `64 KiB` each, which is appropriate for throughput tests but too large for `128k` idle connections. Tune `NacelleConfig::with_read_buffer_capacity(...)` and `with_response_buffer_capacity(...)` per SKU so:

```text
max_connections * (read_buffer_capacity + response_buffer_capacity)
```

fits inside the process/container memory budget with room left for handlers, backend pools, and response bodies.

## Observability

Nacelle emits structured `tracing` events for listener, connection, request completion, and request failure events. Enable the `otel` feature to also record OpenTelemetry metrics:

- `nacelle.connections`
- `nacelle.connections.active`
- `nacelle.requests`
- `nacelle.requests.active`
- `nacelle.streaming_tasks.active`
- `nacelle.memory.used_bytes`
- `nacelle.request_errors`
- `nacelle.rejections`
- `nacelle.timeouts`
- `nacelle.shutdown_events`
- `nacelle.connection_aborts`
- `nacelle.request_bytes`
- `nacelle.response_bytes`
- `nacelle.request_duration_ms`

Production notes:

- [Getting started](docs/tutorials/getting-started.md)
- [Architecture](docs/topics/architecture.md)
- [Operations](docs/topics/operations.md)
- [HTTP hardening](docs/how-to/harden-http.md)
- [Production configuration](docs/how-to/configure-production.md)
- [Stress testing](docs/how-to/run-stress-tests.md)
- [Security scanning](docs/how-to/security-scanning.md)
- [Performance tuning](docs/how-to/compare-performance.md)
- [API stability](docs/reference/api-stability.md)

Generate the mdBook narrative documentation site:

```bash
mdbook build
```

On Windows, the build script installs mdBook if needed:

```powershell
.\scripts\build-book.ps1
```

The mdBook source follows a Django-style organization: tutorials, topic guides,
how-to guides, and reference. The generated output is written to
`target/docs/book`.

Generate Rust API reference with `cargo doc`:

```bash
cargo doc --workspace --all-features --no-deps
```

Or on Windows:

```powershell
.\scripts\build-rustdoc.ps1
```

Internal readiness plans, assessments, and checklists live under `docs/internal`
and are excluded from generated public documentation sites.

Exporter/subscriber setup stays in the application so production can choose OTLP, stdout, Prometheus, or another pipeline.

For custom telemetry systems, implement `NacelleTelemetrySink` and attach it
with `NacelleTelemetry::with_sink(...)`.

## Building

```bash
cargo build --release

# TCP echo
cargo run --features reference_protocol --example echo -- 127.0.0.1:8080

# HTTP echo
cargo run --no-default-features --features http --example http_echo -- 127.0.0.1:8080

# HTTPS echo with an ephemeral self-signed certificate
cargo run --no-default-features --features http,tls-self-signed --example tls_http_echo -- 127.0.0.1:8443

# TCP echo with an ephemeral self-signed certificate
cargo run --features reference_protocol,tls-self-signed --example tls_echo -- 127.0.0.1:8443

# TCP and HTTP with one shared handler and one host
cargo run --features reference_protocol,http --example dual_echo -- 127.0.0.1:8080 127.0.0.1:8081
```

## Stress Harness

```bash
cargo run --release --package nacelle-stress-server --bin tokio-server

# If ./config.toml exists, the stress server loads it automatically. The
# checked-in root config enables self-signed TCP TLS for local runs.

# Or load server limits and buffer sizing from TOML:
cargo run --release --package nacelle-stress-server --bin tokio-server -- \
  --config examples/nacelle-stress-server/config.example.toml

# The stress server default build includes TCP TLS support. Plain TCP
# remains the runtime default; add --tls-self-signed to serve with an ephemeral
# self-signed certificate.
cargo run --release --package nacelle-stress-server --bin tokio-server -- \
  --tls-self-signed

# In another shell:
cargo run --release --package nacelle-stress-test -- \
  --tls-insecure \
  --connections 32 \
  --pipeline 16 \
  --duration-secs 15
```

The optional reference protocol contract is documented in [docs/reference/protocol.md](docs/reference/protocol.md).

Authentication and compression are not implemented in this prototype.

## License

See LICENSE file.
