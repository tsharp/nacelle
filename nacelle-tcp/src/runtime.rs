//! Tokio raw TCP listener helpers.

use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use crate::protocol::Protocol;
use crate::server::NacelleServer;
use nacelle_core::error::NacelleError;
use nacelle_core::handler::Handler;
use nacelle_core::lifecycle::{NacelleDrainDeadline, NacelleShutdownToken};
use nacelle_core::request::RequestMetadata;
use nacelle_core::telemetry::{NacelleTelemetry, NacelleTelemetryEventKind, NacelleTransport};

/// Listen on `addr` and serve each accepted TCP connection in its own task.
pub async fn serve_tcp<Req, P, H>(
    server: Arc<NacelleServer<Req, P, H>>,
    addr: SocketAddr,
) -> Result<(), NacelleError>
where
    Req: RequestMetadata + Send + 'static,
    P: Protocol<Req> + Send + Sync + 'static,
    H: Handler,
{
    let (_shutdown, token) = nacelle_core::lifecycle::NacelleShutdown::pair();
    serve_tcp_with_shutdown(server, addr, token).await
}

/// Listen on `addr` until shutdown is requested.
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

#[doc(hidden)]
pub async fn serve_tcp_with_shutdown_deadline<Req, P, H>(
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

#[doc(hidden)]
pub async fn serve_tcp_listener_with_shutdown_deadline<Req, P, H>(
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
                let (stream, peer_addr) = accepted?;
        let _ = stream.set_nodelay(true);
        let connection_permit = match server.runtime_state().acquire_connection_for_peer(peer_addr.ip()) {
            Ok(permit) => permit,
            Err(error) => {
                server
                    .telemetry()
                    .connection_rejected(NacelleTransport::RawTcp, connection_rejection_reason(&error));
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
    server.telemetry().shutdown_event(
        NacelleTelemetryEventKind::ListenerStoppedAccepting,
        NacelleTransport::RawTcp,
    );
    drain_connection_tasks(
        connections,
        drain_deadline.get(),
        "raw_tcp",
        server.telemetry().clone(),
    )
    .await;
    Ok(())
}

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

fn connection_rejection_reason(error: &NacelleError) -> &'static str {
    match error {
        NacelleError::ResourceLimit(reason) => reason,
        _ => "connections",
    }
}

async fn drain_connection_tasks(
    mut connections: tokio::task::JoinSet<Result<(), NacelleError>>,
    drain_timeout: Duration,
    transport: &'static str,
    telemetry: NacelleTelemetry,
) {
    telemetry.shutdown_event(
        NacelleTelemetryEventKind::DrainStarted,
        NacelleTransport::RawTcp,
    );
    let drain = async {
        while let Some(result) = connections.join_next().await {
            log_connection_result(Some(result));
        }
    };

    if tokio::time::timeout(drain_timeout, drain).await.is_ok() {
        tracing::info!(target: "nacelle", transport, "connection drain completed");
        telemetry.shutdown_event(
            NacelleTelemetryEventKind::DrainCompleted,
            NacelleTransport::RawTcp,
        );
        return;
    }

    let aborted = connections.len();
    tracing::warn!(target: "nacelle", transport, aborted, "connection drain timed out; aborting active tasks");
    telemetry.shutdown_event(
        NacelleTelemetryEventKind::DrainTimedOut,
        NacelleTransport::RawTcp,
    );
    telemetry.connections_aborted(NacelleTransport::RawTcp, aborted);
    connections.abort_all();
    while let Some(result) = connections.join_next().await {
        log_connection_result(Some(result));
    }
}
