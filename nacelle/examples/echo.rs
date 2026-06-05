use std::sync::Arc;

use bytes::BytesMut;
use nacelle::{
    FrameRequest, LengthDelimitedProtocol, NacelleError, NacelleServer, RequestBody,
    ResponseWriter, handler_fn,
};

struct EchoService;

#[tokio::main(flavor = "multi_thread")]
async fn main() -> Result<(), NacelleError> {
    let addr = std::env::args()
        .nth(1)
        .unwrap_or_else(|| "127.0.0.1:8080".to_string())
        .parse()
        .map_err(NacelleError::protocol)?;

    let server = NacelleServer::<EchoService, FrameRequest, ()>::builder()
        .service(EchoService)
        .protocol(LengthDelimitedProtocol)
        .handler(handler_fn(
            |_svc: Arc<EchoService>,
             req: FrameRequest,
             mut body: RequestBody,
             response: ResponseWriter| async move {
                let mut echoed = BytesMut::new();
                while let Some(chunk) = body.next_chunk().await {
                    echoed.extend_from_slice(&chunk?);
                }
                if req.opcode != 1 {
                    return Err(NacelleError::handler(std::io::Error::other(format!(
                        "unknown opcode {}",
                        req.opcode
                    ))));
                }
                response.write_bytes(echoed.freeze())?;
                Ok(())
            },
        ))
        .build()?;

    println!("nacelle echo server listening on {addr}");
    server.serve_tcp(addr).await
}
