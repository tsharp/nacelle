//! Tokio runtime helpers.

use std::future::Future;
use std::pin::Pin;
use std::task::{Context, Poll};

pub use tokio::task::JoinError;

/// A join handle whose output is always `Result<T, JoinError>`.
pub struct JoinHandle<T>(tokio::task::JoinHandle<T>);

impl<T> Future for JoinHandle<T> {
    type Output = Result<T, JoinError>;

    fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        Pin::new(&mut self.0).poll(cx)
    }
}

/// Spawn a `Send + 'static` future onto Tokio.
pub fn spawn<F>(future: F) -> JoinHandle<F::Output>
where
    F: Future + Send + 'static,
    F::Output: Send + 'static,
{
    JoinHandle(tokio::spawn(future))
}

#[cfg(feature = "raw_tcp")]
use std::net::SocketAddr;
#[cfg(feature = "raw_tcp")]
use std::sync::Arc;

#[cfg(feature = "raw_tcp")]
use crate::error::NacelleError;
#[cfg(feature = "raw_tcp")]
use crate::handler::Handler;
#[cfg(feature = "raw_tcp")]
use crate::lifecycle::NacelleShutdownToken;
#[cfg(feature = "raw_tcp")]
use crate::protocol::Protocol;
#[cfg(feature = "raw_tcp")]
use crate::request::RequestMetadata;
#[cfg(feature = "raw_tcp")]
use crate::server::NacelleServer;

/// Listen on `addr` and serve each accepted TCP connection in its own task.
#[cfg(feature = "raw_tcp")]
pub async fn serve_tcp<Req, P, H>(
    server: Arc<NacelleServer<Req, P, H>>,
    addr: SocketAddr,
) -> Result<(), NacelleError>
where
    Req: RequestMetadata + Send + 'static,
    P: Protocol<Req> + Send + Sync + 'static,
    H: Handler,
{
    let (_shutdown, token) = crate::lifecycle::NacelleShutdown::pair();
    serve_tcp_with_shutdown(server, addr, token).await
}

/// Listen on `addr` until shutdown is requested.
#[cfg(feature = "raw_tcp")]
pub async fn serve_tcp_with_shutdown<Req, P, H>(
    server: Arc<NacelleServer<Req, P, H>>,
    addr: SocketAddr,
    mut shutdown: NacelleShutdownToken,
) -> Result<(), NacelleError>
where
    Req: RequestMetadata + Send + 'static,
    P: Protocol<Req> + Send + Sync + 'static,
    H: Handler,
{
    let listener = tokio::net::TcpListener::bind(addr).await?;
    loop {
        let (stream, _) = tokio::select! {
            biased;
            _ = shutdown.changed() => break,
            accepted = listener.accept() => accepted?,
        };
        let _ = stream.set_nodelay(true);
        let connection_permit = match server.runtime_state().acquire_connection_tracked() {
            Ok(permit) => permit,
            Err(error) => {
                tracing::warn!(
                    target: "nacelle",
                    transport = "raw_tcp",
                    error = %error,
                    "connection rejected"
                );
                continue;
            }
        };
        let server = server.clone();
        tokio::spawn(async move {
            let _connection_permit = connection_permit;
            let _ = server.serve_io_without_connection_limit(stream).await;
        });
    }
    Ok(())
}
