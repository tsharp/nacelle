# Nacelle

**Nacelle is an experimental high-performance Rust runtime for building custom binary protocol servers with framed streaming request processing, opcode-based handler dispatch, and minimal head decode.**

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
    .register_handler(
        1, // opcode
        handler_fn(|svc: Arc<MyService>, 
                    _req: FrameRequest,
                    mut body: RequestBody,
                    response: ResponseWriter| async move {
            // Consume request body
            while let Some(chunk) = body.next_chunk().await {
                let _ = chunk?;
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
- `runtime` (default) — Tokio runtime integration  
- `metrics` — Metrics collection hooks
- `tracing` — Distributed tracing support
- `tls` — TLS via rustls
- `auth` — Authentication primitives
- `compression` — Compression support

## Building

```bash
cargo build --release

# Run stress benchmark
cargo run --release --bin stress -- \
  --connections 32 \
  --pipeline 16 \
  --duration-secs 15
```

## Performance

Built for high-throughput workloads. Includes Criterion benchmarks for frame encoding/decoding and handler dispatch.

## License

See LICENSE file.
