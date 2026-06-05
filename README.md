# Nacelle

**Nacelle is an experimental Tokio-based Rust library for streaming application handlers across multiple transports.**

The current transports are:

- `raw_tcp` (default) — custom protocol transport over TCP
- `reference_protocol` — optional length-delimited example protocol
- `http` — Hyper HTTP/1 server transport
- `otel` — OpenTelemetry metrics API integration
- `tower` — adapter for `tower::Service<NacelleRequest>`

Both transports call the same app-facing handler shape:

```rust
async fn handle(request: NacelleRequest) -> Result<NacelleResponse, NacelleError>
```

`NacelleBody` is streaming, so handlers can consume request chunks and return response chunks without forcing full buffering.

## Raw TCP Example

```rust
use nacelle::{
    FrameRequest, LengthDelimitedProtocol, NacelleError, NacelleRequest, NacelleResponse,
    RawTcpServer, handler_fn,
};

let server = RawTcpServer::<FrameRequest, ()>::builder()
    .protocol(LengthDelimitedProtocol)
    .handler(handler_fn(|mut request: NacelleRequest| async move {
        let opcode = request.raw_tcp_opcode().unwrap_or_default();
        if opcode != 1 {
            while let Some(chunk) = request.body.next_chunk().await {
                let _ = chunk?;
            }
            return Err(NacelleError::handler(std::io::Error::other(
                format!("unknown opcode {opcode}"),
            )));
        }
        Ok(NacelleResponse::raw_tcp(request.body))
    }))
    .build()?;

server.serve_tcp("127.0.0.1:8080".parse()?).await?;
```

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
            Ok(NacelleResponse::empty_raw_tcp())
        }
    }
});
```

## Multi-Port Host

`NacelleHost` centralizes listener setup while each server keeps its concrete protocol and handler types:

```rust
let telemetry = NacelleTelemetry::default();
let raw_docs = RawTcpServer::<FrameRequest, ()>::builder()
    .protocol(LengthDelimitedProtocol)
    .telemetry(telemetry.clone())
    .handler(handler.clone())
    .build()?;
let http_api = HyperServer::new(handler).with_telemetry(telemetry.clone());

let mut host = NacelleHost::new().with_telemetry(telemetry);
host.enable_raw_tcp("docs-reference", "127.0.0.1:8080".parse()?, raw_docs)
    .enable_http("http-api", "127.0.0.1:8081".parse()?, http_api);
host.wait().await?;
```

Custom protocols use the same `RawTcpServer::<YourRequest, ()>::builder().protocol(your_protocol)` path, so multiple raw TCP protocols can listen on different ports.

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

For very high connection counts, size buffers deliberately. The default raw TCP read and response buffers are `64 KiB` each, which is appropriate for throughput tests but too large for `128k` idle connections. Tune `NacelleConfig::with_read_buffer_capacity(...)` and `with_response_buffer_capacity(...)` per SKU so:

```text
max_connections * (read_buffer_capacity + response_buffer_capacity)
```

fits inside the process/container memory budget with room left for handlers, backend pools, and response bodies.

## Observability

Nacelle emits structured `tracing` events for listener, connection, request completion, and request failure events. Enable the `otel` feature to also record OpenTelemetry metrics:

- `nacelle.connections`
- `nacelle.requests`
- `nacelle.request_errors`
- `nacelle.request_bytes`
- `nacelle.response_bytes`
- `nacelle.request_duration_ms`

Exporter/subscriber setup stays in the application so production can choose OTLP, stdout, Prometheus, or another pipeline.

## Building

```bash
cargo build --release

# Raw TCP echo
cargo run --features reference_protocol --example echo -- 127.0.0.1:8080

# HTTP echo
cargo run --no-default-features --features http --example http_echo -- 127.0.0.1:8080

# Raw TCP and HTTP with one shared handler and one host
cargo run --features reference_protocol,http --example dual_echo -- 127.0.0.1:8080 127.0.0.1:8081
```

## Stress Harness

```bash
cargo run --release --package nacelle-stress-server --bin tokio-server

# In another shell:
cargo run --release --package nacelle-stress-test -- \
  --connections 32 \
  --pipeline 16 \
  --duration-secs 15
```

The optional reference protocol contract is documented in [docs/PROTOCOL.md](docs/PROTOCOL.md).

TLS, authentication, and compression are not implemented in this prototype.

## License

See LICENSE file.
