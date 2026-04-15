#[cfg(feature = "tcp")]
use std::net::SocketAddr;
#[cfg(feature = "tcp")]
use std::sync::Arc;

#[cfg(feature = "tcp")]
use tokio::net::TcpListener;

#[cfg(feature = "tcp")]
use crate::error::NacelleError;
#[cfg(feature = "tcp")]
use crate::protocol::Protocol;
#[cfg(feature = "tcp")]
use crate::request::RequestMetadata;
#[cfg(feature = "tcp")]
use crate::server::NacelleServer;

#[cfg(feature = "tcp")]
pub async fn serve_tcp<Svc, Req, P>(
    server: Arc<NacelleServer<Svc, Req, P>>,
    addr: SocketAddr,
) -> Result<(), NacelleError>
where
    Svc: Send + Sync + 'static,
    Req: RequestMetadata + Send + 'static,
    P: Protocol<Req> + Send + Sync + 'static,
{
    let listener = TcpListener::bind(addr).await?;
    loop {
        let (stream, _) = listener.accept().await?;
        // Disable Nagle's algorithm so small response frames are sent immediately
        // rather than being held for up to 200 ms waiting for more data to coalesce.
        let _ = stream.set_nodelay(true);
        let server = server.clone();
        tokio::spawn(async move {
            let _ = server.serve_io(stream).await;
        });
    }
}
