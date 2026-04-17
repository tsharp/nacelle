//! Tokio runtime backend.

use std::future::Future;
use std::pin::Pin;
use std::task::{Context, Poll};

pub use tokio::task::JoinError;

/// A join handle whose output is always `Result<T, JoinError>`, matching the
/// interface of every other runtime backend in this crate.
pub struct JoinHandle<T>(tokio::task::JoinHandle<T>);

impl<T> Future for JoinHandle<T> {
    type Output = Result<T, JoinError>;

    fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        Pin::new(&mut self.0).poll(cx)
    }
}

/// Spawn a `Send + 'static` future onto the tokio multi-thread scheduler.
pub fn spawn<F>(future: F) -> JoinHandle<F::Output>
where
    F: Future + Send + 'static,
    F::Output: Send + 'static,
{
    JoinHandle(tokio::spawn(future))
}

// ── TCP accept loop ───────────────────────────────────────────────────────────

#[cfg(feature = "tcp")]
use std::net::SocketAddr;
#[cfg(feature = "tcp")]
use std::sync::Arc;

#[cfg(feature = "tcp")]
use crate::error::NacelleError;
#[cfg(feature = "tcp")]
use crate::protocol::Protocol;
#[cfg(feature = "tcp")]
use crate::request::RequestMetadata;
#[cfg(feature = "tcp")]
use crate::server::NacelleServer;

/// Listen on `addr` and serve each accepted TCP connection using the tokio
/// work-stealing scheduler.  Each connection is driven in its own task.
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
