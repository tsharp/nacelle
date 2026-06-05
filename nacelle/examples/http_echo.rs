use std::sync::Arc;

use bytes::BytesMut;
use http::StatusCode;
use nacelle::{HyperServer, NacelleError, NacelleResponse, handler_fn};

struct EchoService;

#[tokio::main(flavor = "multi_thread")]
async fn main() -> Result<(), NacelleError> {
    let addr = std::env::args()
        .nth(1)
        .unwrap_or_else(|| "127.0.0.1:8080".to_string())
        .parse()
        .map_err(NacelleError::protocol)?;

    let server = HyperServer::new(
        EchoService,
        handler_fn(|_svc: Arc<EchoService>, mut request| async move {
            let mut echoed = BytesMut::new();
            while let Some(chunk) = request.body.next_chunk().await {
                echoed.extend_from_slice(&chunk?);
            }
            Ok(NacelleResponse::http_bytes(StatusCode::OK, echoed.freeze()))
        }),
    );

    println!("nacelle HTTP echo server listening on {addr}");
    server.serve(addr).await
}
