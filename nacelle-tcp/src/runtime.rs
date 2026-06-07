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
#[cfg(feature = "tls")]
use nacelle_core::tls::NacelleTlsConfig;

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

#[cfg(feature = "tls")]
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

#[cfg(feature = "tls")]
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

#[cfg(feature = "tls")]
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

#[cfg(feature = "tls")]
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
    let listener = tokio::net::TcpListener::bind(addr).await?;
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

#[cfg(feature = "tls")]
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
                let acceptor = tokio_rustls::TlsAcceptor::from(tls_config.server_config());
                connections.spawn(async move {
                    let _connection_permit = connection_permit;
                    let stream = match tokio::time::timeout(handshake_timeout, acceptor.accept(stream)).await {
                        Ok(Ok(stream)) => stream,
                        Ok(Err(error)) => return Err(NacelleError::Io(error)),
                        Err(_) => {
                            server
                                .telemetry()
                                .timeout(NacelleTransport::RawTcp, "tls_handshake");
                            return Err(NacelleError::Timeout("tls_handshake"));
                        }
                    };
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

#[cfg(all(test, feature = "tls-self-signed"))]
mod tests {
    use super::*;

    use bytes::{Bytes, BytesMut};
    use nacelle_core::handler::handler_fn;
    use nacelle_core::request::{NacelleRequest, RawTcpRequestMeta, RequestMetadata};
    use nacelle_core::response::NacelleResponse;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    use crate::protocol::{DecodedRequest, Protocol};
    use crate::server::RawTcpServer;

    #[derive(Debug)]
    struct TestRequest;

    impl RequestMetadata for TestRequest {
        fn opcode(&self) -> u64 {
            1
        }

        fn raw_tcp_meta(&self, body_len: usize) -> RawTcpRequestMeta {
            RawTcpRequestMeta {
                request_id: None,
                opcode: 1,
                flags: 0,
                body_len,
            }
        }
    }

    struct TestProtocol;

    impl Protocol<TestRequest> for TestProtocol {
        type ResponseContext = ();
        type ErrorContext = ();

        fn decode_head(
            &self,
            src: &mut BytesMut,
            _max_frame_len: usize,
        ) -> Result<Option<DecodedRequest<TestRequest>>, NacelleError> {
            if src.is_empty() {
                return Ok(None);
            }
            Ok(Some(DecodedRequest {
                request: TestRequest,
                body_len: 1,
            }))
        }

        fn response_context(&self, _req: &TestRequest) -> Self::ResponseContext {}

        fn error_context(&self, _req: &TestRequest) -> Self::ErrorContext {}

        fn encode_response_chunk(
            &self,
            _context: &mut Self::ResponseContext,
            chunk: Bytes,
            dst: &mut BytesMut,
        ) -> Result<(), NacelleError> {
            dst.extend_from_slice(&chunk);
            Ok(())
        }

        fn encode_response_end(
            &self,
            _context: &mut Self::ResponseContext,
            _dst: &mut BytesMut,
        ) -> Result<(), NacelleError> {
            Ok(())
        }

        fn encode_error(
            &self,
            _context: Option<&Self::ErrorContext>,
            _error: &NacelleError,
            _dst: &mut BytesMut,
        ) -> Result<(), NacelleError> {
            Ok(())
        }
    }

    #[tokio::test]
    async fn raw_tcp_tls_self_signed_server_accepts_request() {
        let generated = nacelle_core::tls::NacelleTlsConfig::self_signed(["localhost"])
            .expect("self-signed tls");
        let certificate =
            nacelle_core::tls::parse_pem_certificates(generated.certificate_pem.as_bytes())
                .expect("certificate should parse")
                .remove(0);
        let mut roots = rustls::RootCertStore::empty();
        roots.add(certificate).expect("root cert should add");
        let client_config = rustls::ClientConfig::builder()
            .with_root_certificates(roots)
            .with_no_client_auth();
        let connector = tokio_rustls::TlsConnector::from(Arc::new(client_config));

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("listener should bind");
        let addr = listener.local_addr().expect("listener should have addr");
        let (shutdown, token) = nacelle_core::lifecycle::NacelleShutdown::pair();
        let server = RawTcpServer::<TestRequest, ()>::builder()
            .protocol(TestProtocol)
            .handler(handler_fn(|_request: NacelleRequest| async move {
                Ok(NacelleResponse::raw_tcp_bytes("ok"))
            }))
            .build()
            .expect("server should build");
        let server_task = tokio::spawn(serve_tcp_tls_listener_with_shutdown_deadline(
            Arc::new(server),
            listener,
            generated.tls_config,
            token,
            NacelleDrainDeadline::new(Duration::from_millis(25)),
        ));

        let stream = tokio::net::TcpStream::connect(addr)
            .await
            .expect("client should connect");
        let server_name =
            rustls::pki_types::ServerName::try_from("localhost").expect("valid server name");
        let mut client = connector
            .connect(server_name, stream)
            .await
            .expect("tls should connect");
        client
            .write_all(&[0x01])
            .await
            .expect("request should write");
        let mut response = [0_u8; 2];
        client
            .read_exact(&mut response)
            .await
            .expect("response should read");
        assert_eq!(&response, b"ok");

        shutdown.shutdown();
        tokio::time::timeout(Duration::from_secs(1), server_task)
            .await
            .expect("server should stop")
            .expect("server task should join")
            .expect("server should exit");
    }
}
