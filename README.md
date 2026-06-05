# Nacelle

**Nacelle is an experimental Tokio-based Rust library for streaming application handlers across multiple transports.**

The current transports are:

- `raw_tcp` (default) — custom protocol transport over TCP
- `reference_protocol` — optional length-delimited example protocol
- `http` — Hyper HTTP/1 server transport
- `tower` — adapter for `tower::Service<NacelleRequest>`

Both transports call the same app-facing handler shape:

```rust
async fn handle(request: NacelleRequest) -> Result<NacelleResponse, NacelleError>
```

`NacelleBody` is streaming, so handlers can consume request chunks and return response chunks without forcing full buffering.

## Raw TCP Example

```rust
use std::sync::Arc;

use nacelle::{
    FrameRequest, LengthDelimitedProtocol, NacelleError, NacelleResponse,
    RawTcpServer, handler_fn,
};

struct MyService;

let server = RawTcpServer::<MyService, FrameRequest, ()>::builder()
    .service(MyService)
    .protocol(LengthDelimitedProtocol)
    .handler(handler_fn(|_svc: Arc<MyService>, mut request| async move {
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

## Building

```bash
cargo build --release

# Raw TCP echo
cargo run --features reference_protocol --example echo -- 127.0.0.1:8080

# HTTP echo
cargo run --no-default-features --features http --example http_echo -- 127.0.0.1:8080

# Raw TCP and HTTP with one shared handler
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

TLS, authentication, compression, and metrics are not implemented in this prototype.

## License

See LICENSE file.
