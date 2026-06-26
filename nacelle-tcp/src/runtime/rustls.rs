use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use crate::protocol::Protocol;
use crate::server::NacelleServer;
use nacelle_core::error::NacelleError;
use nacelle_core::handler::Handler;
use nacelle_core::lifecycle::{NacelleDrainDeadline, NacelleShutdownToken};
use nacelle_core::request::{NacelleConnectionMeta, NacelleConnectionTlsMeta, RequestMetadata};
use nacelle_core::telemetry::{NacelleTelemetryEventKind, NacelleTransport};
use nacelle_core::tls::NacelleTlsConfig;

use super::common::{
    bind_tcp_listener, connection_rejection_reason, drain_connection_tasks, log_connection_result,
    record_connection_rejection,
};

pub async fn serve_tcp_tls<Req, P, H>(
    server: Arc<NacelleServer<Req, P, H>>,
    addr: SocketAddr,
    tls_config: NacelleTlsConfig,
) -> Result<(), NacelleError>
where
    Req: RequestMetadata + Send + 'static,
    P: Protocol<Req> + Send + Sync + 'static,
    H: Handler,
{
    let (_shutdown, token) = nacelle_core::lifecycle::NacelleShutdown::pair();
    serve_tcp_tls_with_shutdown(server, addr, tls_config, token).await
}

pub async fn serve_tcp_tls_with_shutdown<Req, P, H>(
    server: Arc<NacelleServer<Req, P, H>>,
    addr: SocketAddr,
    tls_config: NacelleTlsConfig,
    shutdown: NacelleShutdownToken,
) -> Result<(), NacelleError>
where
    Req: RequestMetadata + Send + 'static,
    P: Protocol<Req> + Send + Sync + 'static,
    H: Handler,
{
    serve_tcp_tls_with_shutdown_timeout(server, addr, tls_config, shutdown, Duration::from_secs(30))
        .await
}

pub async fn serve_tcp_tls_with_shutdown_timeout<Req, P, H>(
    server: Arc<NacelleServer<Req, P, H>>,
    addr: SocketAddr,
    tls_config: NacelleTlsConfig,
    shutdown: NacelleShutdownToken,
    drain_timeout: Duration,
) -> Result<(), NacelleError>
where
    Req: RequestMetadata + Send + 'static,
    P: Protocol<Req> + Send + Sync + 'static,
    H: Handler,
{
    serve_tcp_tls_with_shutdown_deadline(
        server,
        addr,
        tls_config,
        shutdown,
        NacelleDrainDeadline::new(drain_timeout),
    )
    .await
}

#[doc(hidden)]
pub async fn serve_tcp_tls_with_shutdown_deadline<Req, P, H>(
    server: Arc<NacelleServer<Req, P, H>>,
    addr: SocketAddr,
    tls_config: NacelleTlsConfig,
    shutdown: NacelleShutdownToken,
    drain_deadline: NacelleDrainDeadline,
) -> Result<(), NacelleError>
where
    Req: RequestMetadata + Send + 'static,
    P: Protocol<Req> + Send + Sync + 'static,
    H: Handler,
{
    let listener = bind_tcp_listener(addr, &Default::default())?;
    serve_tcp_tls_listener_with_shutdown_deadline(
        server,
        listener,
        tls_config,
        shutdown,
        drain_deadline,
    )
    .await
}

#[doc(hidden)]
pub async fn serve_tcp_tls_listener_with_shutdown_deadline<Req, P, H>(
    server: Arc<NacelleServer<Req, P, H>>,
    listener: tokio::net::TcpListener,
    tls_config: NacelleTlsConfig,
    mut shutdown: NacelleShutdownToken,
    drain_deadline: NacelleDrainDeadline,
) -> Result<(), NacelleError>
where
    Req: RequestMetadata + Send + 'static,
    P: Protocol<Req> + Send + Sync + 'static,
    H: Handler,
{
    let handshake_timeout = tls_config.handshake_timeout();
    let mut connections = tokio::task::JoinSet::new();
    let local_addr = listener.local_addr().ok();
    loop {
        tokio::select! {
            biased;
            _ = shutdown.changed() => break,
            joined = connections.join_next(), if !connections.is_empty() => {
                log_connection_result(joined, NacelleTransport::new("tcp"));
                continue;
            }
            accepted = listener.accept() => {
                let (stream, peer_addr) = accepted?;
                let _ = stream.set_nodelay(true);
                let connection = NacelleConnectionMeta::tcp(Some(peer_addr), local_addr);
                let connection_permit = match server.runtime_state().acquire_connection_for_peer(peer_addr.ip()) {
                    Ok(permit) => permit,
                    Err(error) => {
                        record_connection_rejection(server.as_ref(), NacelleTransport::new("tcp"), "rustls", &error);
                        server
                            .telemetry()
                            .connection_rejected(NacelleTransport::new("tcp"), connection_rejection_reason(&error));
                        continue;
                    }
                };
                let server = server.clone();
                let acceptor = tokio_rustls::TlsAcceptor::from(tls_config.server_config());
                connections.spawn(async move {
                    let _connection_permit = connection_permit;
                    let stream = match tokio::time::timeout(handshake_timeout, acceptor.accept(stream)).await {
                        Ok(Ok(stream)) => stream,
                        Ok(Err(error)) => return Err(NacelleError::Io(error)),
                        Err(_) => {
                            server
                                .telemetry()
                                .timeout(NacelleTransport::new("tcp"), "tls_handshake");
                            return Err(NacelleError::Timeout("tls_handshake"));
                        }
                    };
                    let connection = connection.with_tls(NacelleConnectionTlsMeta::new("rustls"));
                    server.serve_io_without_connection_limit(stream, connection).await
                });
            }
        }
    }
    server.telemetry().shutdown_event(
        NacelleTelemetryEventKind::ListenerStoppedAccepting,
        NacelleTransport::new("tcp"),
    );
    drain_connection_tasks(
        connections,
        drain_deadline.get(),
        NacelleTransport::new("tcp"),
        server.telemetry().clone(),
    )
    .await;
    Ok(())
}
