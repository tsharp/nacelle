use std::net::SocketAddr;
use std::time::Duration;

use crate::options::NacelleTcpBindOptions;
use crate::protocol::Protocol;
use crate::server::NacelleServer;
use crate::telemetry::NacelleTcpMetricsContext;
use nacelle_core::error::NacelleError;
use nacelle_core::handler::Handler;
use nacelle_core::request::RequestMetadata;
use nacelle_core::telemetry::{NacelleTelemetry, NacelleTelemetryEventKind, NacelleTransport};

pub(super) fn bind_tcp_listener(
    addr: SocketAddr,
    bind_options: &NacelleTcpBindOptions,
) -> std::io::Result<tokio::net::TcpListener> {
    let domain = if addr.is_ipv4() {
        socket2::Domain::IPV4
    } else {
        socket2::Domain::IPV6
    };
    let socket = socket2::Socket::new(domain, socket2::Type::STREAM, Some(socket2::Protocol::TCP))?;
    socket.set_reuse_address(true)?;
    if addr.is_ipv6()
        && let Some(ipv6_only) = bind_options.ipv6_only
    {
        socket.set_only_v6(ipv6_only)?;
    }
    socket.set_nonblocking(true)?;
    socket.bind(&socket2::SockAddr::from(addr))?;
    socket.listen(1024)?;
    let listener: std::net::TcpListener = socket.into();
    tokio::net::TcpListener::from_std(listener)
}

pub(super) fn log_connection_result(
    result: Option<Result<Result<(), NacelleError>, tokio::task::JoinError>>,
    transport: NacelleTransport,
) {
    match result {
        Some(Ok(Ok(()))) | None => {}
        Some(Ok(Err(error))) => {
            tracing::debug!(target: "nacelle", transport = transport.as_str(), error = %error, "connection finished with error");
        }
        Some(Err(error)) => {
            tracing::warn!(target: "nacelle", transport = transport.as_str(), error = %error, "connection task failed");
        }
    }
}

pub(super) fn connection_rejection_reason(error: &NacelleError) -> &'static str {
    match error {
        NacelleError::ResourceLimit(reason) => reason,
        _ => "connections",
    }
}

pub(super) fn record_connection_rejection<Req, P, H>(
    server: &NacelleServer<Req, P, H>,
    transport: NacelleTransport,
    tls: &'static str,
    error: &NacelleError,
) where
    Req: RequestMetadata + Send + 'static,
    P: Protocol<Req> + Send + Sync + 'static,
    H: Handler,
{
    let context = NacelleTcpMetricsContext::new(
        transport,
        server.listener_label(),
        server.protocol().name(),
        tls,
    );
    server.tcp_telemetry().error(&context, "accept", error);
}

pub(super) async fn drain_connection_tasks(
    mut connections: tokio::task::JoinSet<Result<(), NacelleError>>,
    drain_timeout: Duration,
    transport: NacelleTransport,
    telemetry: NacelleTelemetry,
) {
    telemetry.shutdown_event(NacelleTelemetryEventKind::DrainStarted, transport);
    let drain = async {
        while let Some(result) = connections.join_next().await {
            log_connection_result(Some(result), transport);
        }
    };

    if tokio::time::timeout(drain_timeout, drain).await.is_ok() {
        tracing::info!(target: "nacelle", transport = transport.as_str(), "connection drain completed");
        telemetry.shutdown_event(NacelleTelemetryEventKind::DrainCompleted, transport);
        return;
    }

    let aborted = connections.len();
    tracing::warn!(target: "nacelle", transport = transport.as_str(), aborted, "connection drain timed out; aborting active tasks");
    telemetry.shutdown_event(NacelleTelemetryEventKind::DrainTimedOut, transport);
    telemetry.connections_aborted(transport, aborted);
    connections.abort_all();
    while let Some(result) = connections.join_next().await {
        log_connection_result(Some(result), transport);
    }
}
