#[cfg(feature = "tcp")]
use std::net::SocketAddr;
#[cfg(feature = "tcp")]
use std::sync::Arc;

#[cfg(feature = "tcp")]
use tokio::net::TcpListener;

#[cfg(feature = "tcp")]
use crate::error::CascadeError;
#[cfg(feature = "tcp")]
use crate::protocol::Protocol;
#[cfg(feature = "tcp")]
use crate::request::RequestMetadata;
#[cfg(feature = "tcp")]
use crate::server::CascadeServer;

#[cfg(feature = "tcp")]
pub async fn serve_tcp<Svc, Req, P>(
    server: Arc<CascadeServer<Svc, Req, P>>,
    addr: SocketAddr,
) -> Result<(), CascadeError>
where
    Svc: Send + Sync + 'static,
    Req: RequestMetadata + Send + 'static,
    P: Protocol<Req> + Send + Sync + 'static,
{
    let listener = TcpListener::bind(addr).await?;
    loop {
        let (stream, _) = listener.accept().await?;
        let server = server.clone();
        tokio::spawn(async move {
            let _ = server.serve_io(stream).await;
        });
    }
}
