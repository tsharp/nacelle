use bytes::BytesMut;
use nacelle::{
    FrameRequest, LengthDelimitedProtocol, NacelleError, NacelleRequest, NacelleResponse,
    NacelleTlsConfig, RawTcpServer, handler_fn,
};

#[tokio::main(flavor = "multi_thread")]
async fn main() -> Result<(), NacelleError> {
    let addr = std::env::args()
        .nth(1)
        .unwrap_or_else(|| "127.0.0.1:8443".to_string())
        .parse()
        .map_err(NacelleError::protocol)?;

    let generated = NacelleTlsConfig::self_signed(["localhost", "127.0.0.1"])?;
    let server = RawTcpServer::<FrameRequest, ()>::builder()
        .protocol(LengthDelimitedProtocol)
        .handler(handler_fn(|mut request: NacelleRequest| async move {
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
        }))
        .build()?;

    println!("nacelle TLS echo server listening on {addr}");
    server.serve_tcp_tls(addr, generated.tls_config).await
}
