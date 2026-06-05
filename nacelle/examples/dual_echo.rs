use bytes::BytesMut;
use http::StatusCode;
use nacelle::{
    FrameRequest, HyperServer, LengthDelimitedProtocol, NacelleError, NacelleRequest,
    NacelleResponse, RawTcpServer, handler_fn,
};

#[tokio::main(flavor = "multi_thread")]
async fn main() -> Result<(), NacelleError> {
    let raw_tcp_addr = std::env::args()
        .nth(1)
        .unwrap_or_else(|| "127.0.0.1:8080".to_string())
        .parse()
        .map_err(NacelleError::protocol)?;
    let http_addr = std::env::args()
        .nth(2)
        .unwrap_or_else(|| "127.0.0.1:8081".to_string())
        .parse()
        .map_err(NacelleError::protocol)?;

    let handler = handler_fn(|mut request: NacelleRequest| async move {
        if let Some(opcode) = request.raw_tcp_opcode()
            && opcode != 1
        {
            while let Some(chunk) = request.body.next_chunk().await {
                let _ = chunk?;
            }
            return Err(NacelleError::handler(std::io::Error::other(format!(
                "unknown opcode {opcode}"
            ))));
        }

        let is_http = request.http_meta().is_some();
        let mut echoed = BytesMut::new();
        while let Some(chunk) = request.body.next_chunk().await {
            echoed.extend_from_slice(&chunk?);
        }

        if is_http {
            Ok(NacelleResponse::http_bytes(StatusCode::OK, echoed.freeze()))
        } else {
            Ok(NacelleResponse::raw_tcp_bytes(echoed.freeze()))
        }
    });

    let raw_tcp_server = RawTcpServer::<FrameRequest, ()>::builder()
        .protocol(LengthDelimitedProtocol)
        .handler(handler.clone())
        .build()?;
    let http_server = HyperServer::new(handler);

    println!("nacelle raw TCP echo listening on {raw_tcp_addr}");
    println!("nacelle HTTP echo listening on {http_addr}");

    tokio::try_join!(
        raw_tcp_server.serve_tcp(raw_tcp_addr),
        http_server.serve(http_addr),
    )?;

    Ok(())
}
