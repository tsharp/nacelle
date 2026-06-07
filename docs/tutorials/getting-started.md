# Getting started

This tutorial gets a minimal raw TCP service running with the reference
length-delimited protocol.

## Add nacelle

In this workspace, the umbrella crate is `nacelle`. It re-exports the transport
crates and owns the reference protocol:

```rust
use nacelle::{
    FrameRequest, LengthDelimitedProtocol, NacelleError, NacelleRequest,
    NacelleResponse, RawTcpServer, handler_fn,
};
```

## Build a handler

Handlers receive a `NacelleRequest` and return a `NacelleResponse`.

```rust
let handler = handler_fn(|mut request: NacelleRequest| async move {
    while let Some(chunk) = request.body.next_chunk().await {
        let _ = chunk?;
    }

    Ok(NacelleResponse::raw_tcp_bytes("ok"))
});
```

## Start a raw TCP server

```rust
let server = RawTcpServer::<FrameRequest, ()>::builder()
    .protocol(LengthDelimitedProtocol)
    .handler(handler)
    .build()?;

server.serve_tcp("127.0.0.1:8080".parse()?).await?;
# Ok::<(), NacelleError>(())
```

## Next steps

- Read the [architecture guide](../topics/architecture.md) to understand the request path.
- Read [runtime limits and backpressure](../topics/runtime-limits.md) before raising connection counts.
- Use [Run the stress harness](stress-harness.md) to validate a local build.

