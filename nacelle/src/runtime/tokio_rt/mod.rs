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
use crate::protocol::Protocol;
#[cfg(feature = "raw_tcp")]
use crate::request::RequestMetadata;
#[cfg(feature = "raw_tcp")]
use crate::server::NacelleServer;

/// Listen on `addr` and serve each accepted TCP connection in its own task.
#[cfg(feature = "raw_tcp")]
pub async fn serve_tcp<Svc, Req, P>(
    server: Arc<NacelleServer<Svc, Req, P>>,
    addr: SocketAddr,
) -> Result<(), NacelleError>
where
    Svc: Send + Sync + 'static,
    Req: RequestMetadata + Send + 'static,
    P: Protocol<Req> + Send + Sync + 'static,
{
    let listener = tokio::net::TcpListener::bind(addr).await?;
    loop {
        let (stream, _) = listener.accept().await?;
        let _ = stream.set_nodelay(true);
        let server = server.clone();
        tokio::spawn(async move {
            let _ = server.serve_io(stream).await;
        });
    }
}
