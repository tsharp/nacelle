use std::sync::Arc;

use bytes::BytesMut;
use nacelle::{
    FrameRequest, LengthDelimitedProtocol, NacelleError, NacelleRequest, NacelleResponse,
    RawTcpServer, handler_fn,
};

struct EchoService;

#[tokio::main(flavor = "multi_thread")]
async fn main() -> Result<(), NacelleError> {
    let addr = std::env::args()
        .nth(1)
        .unwrap_or_else(|| "127.0.0.1:8080".to_string())
        .parse()
        .map_err(NacelleError::protocol)?;

    let server = RawTcpServer::<EchoService, FrameRequest, ()>::builder()
        .service(EchoService)
        .protocol(LengthDelimitedProtocol)
        .handler(handler_fn(
            |_svc: Arc<EchoService>, mut request: NacelleRequest| async move {
                let opcode = request.raw_tcp_opcode().unwrap_or_default();
                let mut echoed = BytesMut::new();
                while let Some(chunk) = request.body.next_chunk().await {
                    echoed.extend_from_slice(&chunk?);
                }
                if opcode != 1 {
                    return Err(NacelleError::handler(std::io::Error::other(format!(
                        "unknown opcode {}",
                        opcode
                    ))));
                }
                Ok(NacelleResponse::raw_tcp_bytes(echoed.freeze()))
            },
        ))
        .build()?;

    println!("nacelle echo server listening on {addr}");
    server.serve_tcp(addr).await
}
