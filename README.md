# Nacelle

**Nacelle is an experimental Tokio-based Rust library for building custom binary protocol servers with framed streaming request processing, opcode-based handler dispatch, and minimal head decode.**

## Example

```rust
use std::sync::Arc;
use nacelle::{
    NacelleServer, NacelleConfig, handler_fn,
    RequestBody, ResponseWriter, FrameRequest,
    LengthDelimitedProtocol, NacelleError,
};
use bytes::Bytes;

// Service state shared across all requests
struct MyService {
    response_data: Bytes,
}

// Build server with handlers
let server = NacelleServer::<MyService, FrameRequest, _>::builder()
    .service(MyService {
        response_data: Bytes::from_static(b"Hello"),
    })
    .protocol(LengthDelimitedProtocol)
    .handler(
        handler_fn(|svc: Arc<MyService>, 
                    req: FrameRequest,
                    mut body: RequestBody,
                    response: ResponseWriter| async move {
            // Consume request body
            while let Some(chunk) = body.next_chunk().await {
                let _ = chunk?;
            }
            if req.opcode != 1 {
                return Err(NacelleError::handler(std::io::Error::other(
                    format!("unknown opcode {}", req.opcode),
                )));
            }
            // Write response
            response.write_bytes(svc.response_data.clone())?;
            Ok::<_, NacelleError>(())
        }),
    )
    .build()?;

// Serve on TCP
server.serve_tcp("127.0.0.1:8080".parse()?).await?;
```

## Features

- `tcp` (default) — TCP listener and transport
- Tokio-only runtime support
- Length-delimited binary frame protocol
- Single application handler with request metadata
- Streaming request bodies and framed responses

TLS, authentication, compression, metrics, and Tower integration are not implemented in this prototype.

## Building

```bash
cargo build --release

# Run stress benchmark
cargo run --release --package nacelle-stress-server --bin tokio-server

# In another shell:
cargo run --release --package nacelle-stress-test -- \
  --connections 32 \
  --pipeline 16 \
  --duration-secs 15
```

The protocol contract is documented in [docs/PROTOCOL.md](docs/PROTOCOL.md).
A runnable echo server is available with:

```bash
cargo run --example echo -- 127.0.0.1:8080
```

## Performance

Built for high-throughput workloads. Includes Criterion benchmarks for frame encoding/decoding and handler dispatch.

## License

See LICENSE file.
