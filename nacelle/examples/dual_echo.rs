use std::sync::Arc;

use bytes::BytesMut;
use http::StatusCode;
use nacelle::{
    FrameRequest, HyperServer, LengthDelimitedProtocol, NacelleError, NacelleHost, NacelleRequest,
    NacelleResponse, NacelleTelemetry, RawTcpServer, handler_fn,
};

#[derive(Debug)]
struct AppState {
    response_prefix: &'static [u8],
}

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

    let app = Arc::new(AppState {
        response_prefix: b"",
    });
    let telemetry = NacelleTelemetry::default();
    let handler = handler_fn({
        let app = app.clone();
        move |mut request: NacelleRequest| {
            let app = app.clone();
            async move {
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
                echoed.extend_from_slice(app.response_prefix);
                while let Some(chunk) = request.body.next_chunk().await {
                    echoed.extend_from_slice(&chunk?);
                }

                if is_http {
                    Ok(NacelleResponse::http_bytes(StatusCode::OK, echoed.freeze()))
                } else {
                    Ok(NacelleResponse::raw_tcp_bytes(echoed.freeze()))
                }
            }
        }
    });

    let raw_tcp_server = RawTcpServer::<FrameRequest, ()>::builder()
        .protocol(LengthDelimitedProtocol)
        .telemetry(telemetry.clone())
        .handler(handler.clone())
        .build()?;
    let http_server = HyperServer::new(handler).with_telemetry(telemetry.clone());

    println!("nacelle raw TCP echo listening on {raw_tcp_addr}");
    println!("nacelle HTTP echo listening on {http_addr}");

    let mut host = NacelleHost::new().with_telemetry(telemetry);
    host.enable_raw_tcp("raw-echo", raw_tcp_addr, raw_tcp_server)
        .enable_http("http-echo", http_addr, http_server);
    host.wait().await
}
