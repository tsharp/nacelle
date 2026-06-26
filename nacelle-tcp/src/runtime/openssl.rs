use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use openssl::ssl::{NameType, Ssl, SslRef};

use crate::options::{NacelleTcpBindOptions, NacelleTcpOptions};
use crate::protocol::Protocol;
use crate::server::NacelleServer;
use nacelle_core::error::NacelleError;
use nacelle_core::handler::Handler;
use nacelle_core::lifecycle::{NacelleDrainDeadline, NacelleShutdownToken};
use nacelle_core::request::{NacelleConnectionMeta, NacelleConnectionTlsMeta, RequestMetadata};
use nacelle_core::telemetry::{NacelleTelemetryEventKind, NacelleTransport};
use nacelle_core::tls::NacelleOpenSslConfig;

use super::common::{
    bind_tcp_listener, connection_rejection_reason, drain_connection_tasks, log_connection_result,
    record_connection_rejection,
};

pub async fn serve_tcp_openssl<Req, P, H>(
    server: Arc<NacelleServer<Req, P, H>>,
    addr: SocketAddr,
    tls_config: NacelleOpenSslConfig,
) -> Result<(), NacelleError>
where
    Req: RequestMetadata + Send + 'static,
    P: Protocol<Req> + Send + Sync + 'static,
    H: Handler,
{
    let (_shutdown, token) = nacelle_core::lifecycle::NacelleShutdown::pair();
    serve_tcp_openssl_with_shutdown(server, addr, tls_config, token).await
}

pub async fn serve_tcp_openssl_with_shutdown<Req, P, H>(
    server: Arc<NacelleServer<Req, P, H>>,
    addr: SocketAddr,
    tls_config: NacelleOpenSslConfig,
    shutdown: NacelleShutdownToken,
) -> Result<(), NacelleError>
where
    Req: RequestMetadata + Send + 'static,
    P: Protocol<Req> + Send + Sync + 'static,
    H: Handler,
{
    serve_tcp_openssl_with_shutdown_timeout(
        server,
        addr,
        tls_config,
        shutdown,
        Duration::from_secs(30),
    )
    .await
}

pub async fn serve_tcp_openssl_with_shutdown_timeout<Req, P, H>(
    server: Arc<NacelleServer<Req, P, H>>,
    addr: SocketAddr,
    tls_config: NacelleOpenSslConfig,
    shutdown: NacelleShutdownToken,
    drain_timeout: Duration,
) -> Result<(), NacelleError>
where
    Req: RequestMetadata + Send + 'static,
    P: Protocol<Req> + Send + Sync + 'static,
    H: Handler,
{
    serve_tcp_openssl_with_shutdown_deadline(
        server,
        addr,
        tls_config,
        shutdown,
        NacelleDrainDeadline::new(drain_timeout),
    )
    .await
}

pub async fn serve_tcp_openssl_with_options<Req, P, H>(
    server: Arc<NacelleServer<Req, P, H>>,
    addr: SocketAddr,
    tls_config: NacelleOpenSslConfig,
    tcp_options: NacelleTcpOptions,
) -> Result<(), NacelleError>
where
    Req: RequestMetadata + Send + 'static,
    P: Protocol<Req> + Send + Sync + 'static,
    H: Handler,
{
    let (_shutdown, token) = nacelle_core::lifecycle::NacelleShutdown::pair();
    serve_tcp_openssl_with_options_and_shutdown(server, addr, tls_config, tcp_options, token).await
}

pub async fn serve_tcp_openssl_with_options_and_shutdown<Req, P, H>(
    server: Arc<NacelleServer<Req, P, H>>,
    addr: SocketAddr,
    tls_config: NacelleOpenSslConfig,
    tcp_options: NacelleTcpOptions,
    shutdown: NacelleShutdownToken,
) -> Result<(), NacelleError>
where
    Req: RequestMetadata + Send + 'static,
    P: Protocol<Req> + Send + Sync + 'static,
    H: Handler,
{
    serve_tcp_openssl_with_options_and_shutdown_timeout(
        server,
        addr,
        tls_config,
        tcp_options,
        shutdown,
        Duration::from_secs(30),
    )
    .await
}

pub async fn serve_tcp_openssl_with_options_and_shutdown_timeout<Req, P, H>(
    server: Arc<NacelleServer<Req, P, H>>,
    addr: SocketAddr,
    tls_config: NacelleOpenSslConfig,
    tcp_options: NacelleTcpOptions,
    shutdown: NacelleShutdownToken,
    drain_timeout: Duration,
) -> Result<(), NacelleError>
where
    Req: RequestMetadata + Send + 'static,
    P: Protocol<Req> + Send + Sync + 'static,
    H: Handler,
{
    serve_tcp_openssl_with_options_and_shutdown_deadline(
        server,
        addr,
        tls_config,
        tcp_options,
        shutdown,
        NacelleDrainDeadline::new(drain_timeout),
    )
    .await
}

#[doc(hidden)]
pub async fn serve_tcp_openssl_with_shutdown_deadline<Req, P, H>(
    server: Arc<NacelleServer<Req, P, H>>,
    addr: SocketAddr,
    tls_config: NacelleOpenSslConfig,
    shutdown: NacelleShutdownToken,
    drain_deadline: NacelleDrainDeadline,
) -> Result<(), NacelleError>
where
    Req: RequestMetadata + Send + 'static,
    P: Protocol<Req> + Send + Sync + 'static,
    H: Handler,
{
    serve_tcp_openssl_with_bind_options_and_shutdown_deadline(
        server,
        addr,
        tls_config,
        NacelleTcpBindOptions::default(),
        shutdown,
        drain_deadline,
    )
    .await
}

#[doc(hidden)]
pub async fn serve_tcp_openssl_with_options_and_shutdown_deadline<Req, P, H>(
    server: Arc<NacelleServer<Req, P, H>>,
    addr: SocketAddr,
    tls_config: NacelleOpenSslConfig,
    tcp_options: NacelleTcpOptions,
    shutdown: NacelleShutdownToken,
    drain_deadline: NacelleDrainDeadline,
) -> Result<(), NacelleError>
where
    Req: RequestMetadata + Send + 'static,
    P: Protocol<Req> + Send + Sync + 'static,
    H: Handler,
{
    serve_tcp_openssl_with_bind_options_and_shutdown_deadline(
        server,
        addr,
        tls_config,
        NacelleTcpBindOptions::from(tcp_options),
        shutdown,
        drain_deadline,
    )
    .await
}

#[doc(hidden)]
pub async fn serve_tcp_openssl_with_bind_options_and_shutdown_deadline<Req, P, H>(
    server: Arc<NacelleServer<Req, P, H>>,
    addr: SocketAddr,
    tls_config: NacelleOpenSslConfig,
    bind_options: NacelleTcpBindOptions,
    shutdown: NacelleShutdownToken,
    drain_deadline: NacelleDrainDeadline,
) -> Result<(), NacelleError>
where
    Req: RequestMetadata + Send + 'static,
    P: Protocol<Req> + Send + Sync + 'static,
    H: Handler,
{
    let listener = bind_tcp_listener(addr, &bind_options)?;
    serve_tcp_openssl_listener_with_options_and_shutdown_deadline(
        server,
        listener,
        tls_config,
        bind_options.stream,
        shutdown,
        drain_deadline,
    )
    .await
}

#[doc(hidden)]
pub async fn serve_tcp_openssl_listener_with_shutdown_deadline<Req, P, H>(
    server: Arc<NacelleServer<Req, P, H>>,
    listener: tokio::net::TcpListener,
    tls_config: NacelleOpenSslConfig,
    shutdown: NacelleShutdownToken,
    drain_deadline: NacelleDrainDeadline,
) -> Result<(), NacelleError>
where
    Req: RequestMetadata + Send + 'static,
    P: Protocol<Req> + Send + Sync + 'static,
    H: Handler,
{
    serve_tcp_openssl_listener_with_options_and_shutdown_deadline(
        server,
        listener,
        tls_config,
        NacelleTcpOptions::default(),
        shutdown,
        drain_deadline,
    )
    .await
}

#[doc(hidden)]
pub async fn serve_tcp_openssl_listener_with_options_and_shutdown_deadline<Req, P, H>(
    server: Arc<NacelleServer<Req, P, H>>,
    listener: tokio::net::TcpListener,
    tls_config: NacelleOpenSslConfig,
    tcp_options: NacelleTcpOptions,
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
                tcp_options.apply_to_stream(&stream)?;
                let connection = NacelleConnectionMeta::tcp(Some(peer_addr), local_addr);
                let connection_permit = match server.runtime_state().acquire_connection_for_peer(peer_addr.ip()) {
                    Ok(permit) => permit,
                    Err(error) => {
                        record_connection_rejection(server.as_ref(), NacelleTransport::new("tcp"), "openssl", &error);
                        server
                            .telemetry()
                            .connection_rejected(NacelleTransport::new("tcp"), connection_rejection_reason(&error));
                        continue;
                    }
                };
                let server = server.clone();
                let acceptor = tls_config.acceptor();
                connections.spawn(async move {
                    let _connection_permit = connection_permit;
                    let ssl = Ssl::new(acceptor.context()).map_err(NacelleError::protocol)?;
                    let mut stream = tokio_openssl::SslStream::new(ssl, stream)
                        .map_err(NacelleError::protocol)?;
                    match tokio::time::timeout(
                        handshake_timeout,
                        std::pin::Pin::new(&mut stream).accept(),
                    )
                    .await
                    {
                        Ok(Ok(())) => {}
                        Ok(Err(error)) => return Err(NacelleError::protocol(error)),
                        Err(_) => {
                            server
                                .telemetry()
                                .timeout(NacelleTransport::new("tcp"), "tls_handshake");
                            return Err(NacelleError::Timeout("tls_handshake"));
                        }
                    }
                    let connection = connection.with_tls(openssl_tls_meta(stream.ssl()));
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

pub(super) fn openssl_tls_meta(ssl: &SslRef) -> NacelleConnectionTlsMeta {
    let mut meta = NacelleConnectionTlsMeta::new("openssl").with_protocol(ssl.version_str());
    if let Some(cipher) = ssl.current_cipher() {
        meta = meta.with_cipher_suite(cipher.name());
        let bits = cipher.bits();
        if let Ok(secret_bits) = u16::try_from(bits.secret) {
            meta = meta.with_cipher_bits(secret_bits);
        }
        if let Ok(algorithm_bits) = u16::try_from(bits.algorithm) {
            meta = meta.with_cipher_algorithm_bits(algorithm_bits);
        }
    }
    if let Some(server_name) = ssl.servername(NameType::HOST_NAME) {
        meta = meta.with_server_name(server_name);
    }
    meta
}
