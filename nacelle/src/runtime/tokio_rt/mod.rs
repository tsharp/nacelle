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
use std::time::Duration;

#[cfg(feature = "raw_tcp")]
use crate::error::NacelleError;
#[cfg(feature = "raw_tcp")]
use crate::handler::Handler;
#[cfg(feature = "raw_tcp")]
use crate::lifecycle::{NacelleDrainDeadline, NacelleShutdownToken};
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
    shutdown: NacelleShutdownToken,
) -> Result<(), NacelleError>
where
    Req: RequestMetadata + Send + 'static,
    P: Protocol<Req> + Send + Sync + 'static,
    H: Handler,
{
    serve_tcp_with_shutdown_timeout(server, addr, shutdown, Duration::from_secs(30)).await
}

/// Listen on `addr` until shutdown is requested, then drain or abort active
/// connection tasks after `drain_timeout`.
#[cfg(feature = "raw_tcp")]
pub async fn serve_tcp_with_shutdown_timeout<Req, P, H>(
    server: Arc<NacelleServer<Req, P, H>>,
    addr: SocketAddr,
    shutdown: NacelleShutdownToken,
    drain_timeout: Duration,
) -> Result<(), NacelleError>
where
    Req: RequestMetadata + Send + 'static,
    P: Protocol<Req> + Send + Sync + 'static,
    H: Handler,
{
    serve_tcp_with_shutdown_deadline(
        server,
        addr,
        shutdown,
        NacelleDrainDeadline::new(drain_timeout),
    )
    .await
}

#[cfg(feature = "raw_tcp")]
pub(crate) async fn serve_tcp_with_shutdown_deadline<Req, P, H>(
    server: Arc<NacelleServer<Req, P, H>>,
    addr: SocketAddr,
    shutdown: NacelleShutdownToken,
    drain_deadline: NacelleDrainDeadline,
) -> Result<(), NacelleError>
where
    Req: RequestMetadata + Send + 'static,
    P: Protocol<Req> + Send + Sync + 'static,
    H: Handler,
{
    let listener = tokio::net::TcpListener::bind(addr).await?;
    serve_tcp_listener_with_shutdown_deadline(server, listener, shutdown, drain_deadline).await
}

#[cfg(feature = "raw_tcp")]
pub(crate) async fn serve_tcp_listener_with_shutdown_deadline<Req, P, H>(
    server: Arc<NacelleServer<Req, P, H>>,
    listener: tokio::net::TcpListener,
    mut shutdown: NacelleShutdownToken,
    drain_deadline: NacelleDrainDeadline,
) -> Result<(), NacelleError>
where
    Req: RequestMetadata + Send + 'static,
    P: Protocol<Req> + Send + Sync + 'static,
    H: Handler,
{
    let mut connections = tokio::task::JoinSet::new();
    loop {
        tokio::select! {
            biased;
            _ = shutdown.changed() => break,
            joined = connections.join_next(), if !connections.is_empty() => {
                log_connection_result(joined);
                continue;
            }
            accepted = listener.accept() => {
                let (stream, _) = accepted?;
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
                connections.spawn(async move {
            let _connection_permit = connection_permit;
                    server.serve_io_without_connection_limit(stream).await
        });
            }
        }
    }
    tracing::info!(target: "nacelle", transport = "raw_tcp", "listener stopped accepting");
    drain_connection_tasks(connections, drain_deadline.get(), "raw_tcp").await;
    Ok(())
}

#[cfg(feature = "raw_tcp")]
fn log_connection_result(result: Option<Result<Result<(), NacelleError>, tokio::task::JoinError>>) {
    match result {
        Some(Ok(Ok(()))) | None => {}
        Some(Ok(Err(error))) => {
            tracing::debug!(target: "nacelle", transport = "raw_tcp", error = %error, "connection finished with error");
        }
        Some(Err(error)) => {
            tracing::warn!(target: "nacelle", transport = "raw_tcp", error = %error, "connection task failed");
        }
    }
}

#[cfg(feature = "raw_tcp")]
async fn drain_connection_tasks(
    mut connections: tokio::task::JoinSet<Result<(), NacelleError>>,
    drain_timeout: Duration,
    transport: &'static str,
) {
    let drain = async {
        while let Some(result) = connections.join_next().await {
            log_connection_result(Some(result));
        }
    };

    if tokio::time::timeout(drain_timeout, drain).await.is_ok() {
        tracing::info!(target: "nacelle", transport, "connection drain completed");
        return;
    }

    let aborted = connections.len();
    tracing::warn!(target: "nacelle", transport, aborted, "connection drain timed out; aborting active tasks");
    connections.abort_all();
    while let Some(result) = connections.join_next().await {
        log_connection_result(Some(result));
    }
}

#[cfg(all(test, feature = "reference_protocol"))]
mod tests {
    use super::*;

    use crate::handler::handler_fn;
    use crate::reference_protocol::{FrameRequest, LengthDelimitedProtocol};
    use crate::response::NacelleResponse;

    #[tokio::test]
    async fn raw_tcp_shutdown_aborts_after_drain_deadline() {
        let runtime_state = crate::limits::NacelleRuntimeState::default();
        let server = crate::server::NacelleServer::<FrameRequest, ()>::builder()
            .protocol(LengthDelimitedProtocol)
            .runtime_state(runtime_state.clone())
            .handler(handler_fn(|_request| async move {
                tokio::time::sleep(Duration::from_secs(60)).await;
                Ok(NacelleResponse::empty_raw_tcp())
            }))
            .build()
            .expect("server should build");

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("listener should bind");
        let addr = listener.local_addr().expect("listener should have addr");
        let (shutdown, token) = crate::lifecycle::NacelleShutdown::pair();
        let server_task = tokio::spawn(serve_tcp_listener_with_shutdown_deadline(
            Arc::new(server),
            listener,
            token,
            NacelleDrainDeadline::new(Duration::from_millis(10)),
        ));

        let _client = tokio::net::TcpStream::connect(addr)
            .await
            .expect("client should connect");
        wait_for_active_connections(&runtime_state, 1).await;

        shutdown.shutdown();

        tokio::time::timeout(Duration::from_secs(1), server_task)
            .await
            .expect("server should stop before test timeout")
            .expect("server task should join")
            .expect("server should exit cleanly");
        assert_eq!(runtime_state.active_connections(), 0);
    }

    async fn wait_for_active_connections(
        runtime_state: &crate::limits::NacelleRuntimeState,
        expected: usize,
    ) {
        for _ in 0..100 {
            if runtime_state.active_connections() == expected {
                return;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
        panic!(
            "active connections did not reach {expected}; observed {}",
            runtime_state.active_connections()
        );
    }
}
