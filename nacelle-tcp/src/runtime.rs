//! Tokio TCP listener helpers.

use std::net::SocketAddr;
#[cfg(unix)]
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

#[cfg(feature = "openssl")]
use crate::options::NacelleTlsDetectionOptions;
#[cfg(unix)]
use crate::options::NacelleUnixSocketOptions;
use crate::options::{NacelleTcpBindOptions, NacelleTcpOptions};
use crate::protocol::Protocol;
use crate::server::NacelleServer;
use nacelle_core::error::NacelleError;
use nacelle_core::handler::Handler;
use nacelle_core::lifecycle::{NacelleDrainDeadline, NacelleShutdownToken};
#[cfg(any(feature = "rustls", feature = "openssl"))]
use nacelle_core::request::NacelleConnectionTlsMeta;
use nacelle_core::request::{NacelleConnectionMeta, RequestMetadata};
use nacelle_core::telemetry::{NacelleTelemetry, NacelleTelemetryEventKind, NacelleTransport};
#[cfg(feature = "openssl")]
use nacelle_core::tls::NacelleOpenSslConfig;
#[cfg(feature = "rustls")]
use nacelle_core::tls::NacelleTlsConfig;
#[cfg(feature = "openssl")]
use openssl::ssl::{NameType, Ssl, SslRef};
#[cfg(unix)]
use tokio::net::UnixListener;

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

/// Listen on `addr` with explicit TCP socket options.
pub async fn serve_tcp_with_options<Req, P, H>(
    server: Arc<NacelleServer<Req, P, H>>,
    addr: SocketAddr,
    tcp_options: NacelleTcpOptions,
) -> Result<(), NacelleError>
where
    Req: RequestMetadata + Send + 'static,
    P: Protocol<Req> + Send + Sync + 'static,
    H: Handler,
{
    let (_shutdown, token) = nacelle_core::lifecycle::NacelleShutdown::pair();
    serve_tcp_with_options_and_shutdown(server, addr, tcp_options, token).await
}

/// Listen on `addr` with explicit TCP socket options until shutdown is requested.
pub async fn serve_tcp_with_options_and_shutdown<Req, P, H>(
    server: Arc<NacelleServer<Req, P, H>>,
    addr: SocketAddr,
    tcp_options: NacelleTcpOptions,
    shutdown: NacelleShutdownToken,
) -> Result<(), NacelleError>
where
    Req: RequestMetadata + Send + 'static,
    P: Protocol<Req> + Send + Sync + 'static,
    H: Handler,
{
    serve_tcp_with_options_and_shutdown_timeout(
        server,
        addr,
        tcp_options,
        shutdown,
        Duration::from_secs(30),
    )
    .await
}

/// Listen on `addr` with explicit TCP socket options, then drain or abort active
/// connection tasks after `drain_timeout`.
pub async fn serve_tcp_with_options_and_shutdown_timeout<Req, P, H>(
    server: Arc<NacelleServer<Req, P, H>>,
    addr: SocketAddr,
    tcp_options: NacelleTcpOptions,
    shutdown: NacelleShutdownToken,
    drain_timeout: Duration,
) -> Result<(), NacelleError>
where
    Req: RequestMetadata + Send + 'static,
    P: Protocol<Req> + Send + Sync + 'static,
    H: Handler,
{
    serve_tcp_with_options_and_shutdown_deadline(
        server,
        addr,
        tcp_options,
        shutdown,
        NacelleDrainDeadline::new(drain_timeout),
    )
    .await
}

#[doc(hidden)]
pub async fn serve_tcp_with_bind_options_and_shutdown_deadline<Req, P, H>(
    server: Arc<NacelleServer<Req, P, H>>,
    addr: SocketAddr,
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
    serve_tcp_listener_with_options_and_shutdown_deadline(
        server,
        listener,
        bind_options.stream,
        shutdown,
        drain_deadline,
    )
    .await
}

/// Listen on a Unix domain socket and serve each accepted connection.
///
/// The socket path is passed directly to Tokio. Existing socket files are not
/// removed automatically.
#[cfg(unix)]
pub async fn serve_unix<Req, P, H>(
    server: Arc<NacelleServer<Req, P, H>>,
    path: impl AsRef<Path>,
) -> Result<(), NacelleError>
where
    Req: RequestMetadata + Send + 'static,
    P: Protocol<Req> + Send + Sync + 'static,
    H: Handler,
{
    let (_shutdown, token) = nacelle_core::lifecycle::NacelleShutdown::pair();
    serve_unix_with_shutdown(server, path, token).await
}

/// Listen on a Unix domain socket until shutdown is requested.
#[cfg(unix)]
pub async fn serve_unix_with_shutdown<Req, P, H>(
    server: Arc<NacelleServer<Req, P, H>>,
    path: impl AsRef<Path>,
    shutdown: NacelleShutdownToken,
) -> Result<(), NacelleError>
where
    Req: RequestMetadata + Send + 'static,
    P: Protocol<Req> + Send + Sync + 'static,
    H: Handler,
{
    serve_unix_with_shutdown_timeout(server, path, shutdown, Duration::from_secs(30)).await
}

/// Listen on a Unix domain socket until shutdown is requested, then drain or
/// abort active connection tasks after `drain_timeout`.
#[cfg(unix)]
pub async fn serve_unix_with_shutdown_timeout<Req, P, H>(
    server: Arc<NacelleServer<Req, P, H>>,
    path: impl AsRef<Path>,
    shutdown: NacelleShutdownToken,
    drain_timeout: Duration,
) -> Result<(), NacelleError>
where
    Req: RequestMetadata + Send + 'static,
    P: Protocol<Req> + Send + Sync + 'static,
    H: Handler,
{
    serve_unix_with_shutdown_deadline(
        server,
        path,
        shutdown,
        NacelleDrainDeadline::new(drain_timeout),
    )
    .await
}

/// Listen on a Unix domain socket with explicit socket-file lifecycle options.
#[cfg(unix)]
pub async fn serve_unix_with_options<Req, P, H>(
    server: Arc<NacelleServer<Req, P, H>>,
    path: impl AsRef<Path>,
    unix_options: NacelleUnixSocketOptions,
) -> Result<(), NacelleError>
where
    Req: RequestMetadata + Send + 'static,
    P: Protocol<Req> + Send + Sync + 'static,
    H: Handler,
{
    let (_shutdown, token) = nacelle_core::lifecycle::NacelleShutdown::pair();
    serve_unix_with_options_and_shutdown(server, path, unix_options, token).await
}

/// Listen on a Unix domain socket with explicit lifecycle options until
/// shutdown is requested.
#[cfg(unix)]
pub async fn serve_unix_with_options_and_shutdown<Req, P, H>(
    server: Arc<NacelleServer<Req, P, H>>,
    path: impl AsRef<Path>,
    unix_options: NacelleUnixSocketOptions,
    shutdown: NacelleShutdownToken,
) -> Result<(), NacelleError>
where
    Req: RequestMetadata + Send + 'static,
    P: Protocol<Req> + Send + Sync + 'static,
    H: Handler,
{
    serve_unix_with_options_and_shutdown_timeout(
        server,
        path,
        unix_options,
        shutdown,
        Duration::from_secs(30),
    )
    .await
}

/// Listen on a Unix domain socket with explicit lifecycle options, then drain
/// or abort active connection tasks after `drain_timeout`.
#[cfg(unix)]
pub async fn serve_unix_with_options_and_shutdown_timeout<Req, P, H>(
    server: Arc<NacelleServer<Req, P, H>>,
    path: impl AsRef<Path>,
    unix_options: NacelleUnixSocketOptions,
    shutdown: NacelleShutdownToken,
    drain_timeout: Duration,
) -> Result<(), NacelleError>
where
    Req: RequestMetadata + Send + 'static,
    P: Protocol<Req> + Send + Sync + 'static,
    H: Handler,
{
    serve_unix_with_options_and_shutdown_deadline(
        server,
        path,
        unix_options,
        shutdown,
        NacelleDrainDeadline::new(drain_timeout),
    )
    .await
}

#[cfg(feature = "rustls")]
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

#[cfg(feature = "rustls")]
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

#[cfg(feature = "rustls")]
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

#[cfg(feature = "openssl")]
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

#[cfg(feature = "openssl")]
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

#[cfg(feature = "openssl")]
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

#[cfg(feature = "openssl")]
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

#[cfg(feature = "openssl")]
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

#[cfg(feature = "openssl")]
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

#[cfg(feature = "openssl")]
pub async fn serve_tcp_optional_openssl<Req, P, H>(
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
    serve_tcp_optional_openssl_with_shutdown(server, addr, tls_config, token).await
}

#[cfg(feature = "openssl")]
pub async fn serve_tcp_optional_openssl_with_shutdown<Req, P, H>(
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
    serve_tcp_optional_openssl_with_options_and_shutdown(
        server,
        addr,
        tls_config,
        NacelleTcpOptions::default(),
        NacelleTlsDetectionOptions::default(),
        shutdown,
    )
    .await
}

#[cfg(feature = "openssl")]
pub async fn serve_tcp_optional_openssl_with_shutdown_timeout<Req, P, H>(
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
    serve_tcp_optional_openssl_with_options_and_shutdown_timeout(
        server,
        addr,
        tls_config,
        NacelleTcpOptions::default(),
        NacelleTlsDetectionOptions::default(),
        shutdown,
        drain_timeout,
    )
    .await
}

#[cfg(feature = "openssl")]
pub async fn serve_tcp_optional_openssl_with_options<Req, P, H>(
    server: Arc<NacelleServer<Req, P, H>>,
    addr: SocketAddr,
    tls_config: NacelleOpenSslConfig,
    tcp_options: NacelleTcpOptions,
    detection_options: NacelleTlsDetectionOptions,
) -> Result<(), NacelleError>
where
    Req: RequestMetadata + Send + 'static,
    P: Protocol<Req> + Send + Sync + 'static,
    H: Handler,
{
    let (_shutdown, token) = nacelle_core::lifecycle::NacelleShutdown::pair();
    serve_tcp_optional_openssl_with_options_and_shutdown(
        server,
        addr,
        tls_config,
        tcp_options,
        detection_options,
        token,
    )
    .await
}

#[cfg(feature = "openssl")]
pub async fn serve_tcp_optional_openssl_with_options_and_shutdown<Req, P, H>(
    server: Arc<NacelleServer<Req, P, H>>,
    addr: SocketAddr,
    tls_config: NacelleOpenSslConfig,
    tcp_options: NacelleTcpOptions,
    detection_options: NacelleTlsDetectionOptions,
    shutdown: NacelleShutdownToken,
) -> Result<(), NacelleError>
where
    Req: RequestMetadata + Send + 'static,
    P: Protocol<Req> + Send + Sync + 'static,
    H: Handler,
{
    serve_tcp_optional_openssl_with_options_and_shutdown_timeout(
        server,
        addr,
        tls_config,
        tcp_options,
        detection_options,
        shutdown,
        Duration::from_secs(30),
    )
    .await
}

#[cfg(feature = "openssl")]
#[allow(clippy::too_many_arguments)]
pub async fn serve_tcp_optional_openssl_with_options_and_shutdown_timeout<Req, P, H>(
    server: Arc<NacelleServer<Req, P, H>>,
    addr: SocketAddr,
    tls_config: NacelleOpenSslConfig,
    tcp_options: NacelleTcpOptions,
    detection_options: NacelleTlsDetectionOptions,
    shutdown: NacelleShutdownToken,
    drain_timeout: Duration,
) -> Result<(), NacelleError>
where
    Req: RequestMetadata + Send + 'static,
    P: Protocol<Req> + Send + Sync + 'static,
    H: Handler,
{
    serve_tcp_optional_openssl_with_options_and_shutdown_deadline(
        server,
        addr,
        tls_config,
        tcp_options,
        detection_options,
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
    serve_tcp_listener_with_options_and_shutdown_deadline(
        server,
        listener,
        NacelleTcpOptions::default(),
        shutdown,
        drain_deadline,
    )
    .await
}

#[cfg(feature = "rustls")]
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

#[cfg(feature = "openssl")]
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

#[cfg(feature = "openssl")]
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

#[cfg(feature = "openssl")]
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

#[cfg(feature = "openssl")]
#[doc(hidden)]
#[allow(clippy::too_many_arguments)]
pub async fn serve_tcp_optional_openssl_with_options_and_shutdown_deadline<Req, P, H>(
    server: Arc<NacelleServer<Req, P, H>>,
    addr: SocketAddr,
    tls_config: NacelleOpenSslConfig,
    tcp_options: NacelleTcpOptions,
    detection_options: NacelleTlsDetectionOptions,
    shutdown: NacelleShutdownToken,
    drain_deadline: NacelleDrainDeadline,
) -> Result<(), NacelleError>
where
    Req: RequestMetadata + Send + 'static,
    P: Protocol<Req> + Send + Sync + 'static,
    H: Handler,
{
    serve_tcp_optional_openssl_with_bind_options_and_shutdown_deadline(
        server,
        addr,
        tls_config,
        NacelleTcpBindOptions::from(tcp_options),
        detection_options,
        shutdown,
        drain_deadline,
    )
    .await
}

#[cfg(feature = "openssl")]
#[doc(hidden)]
#[allow(clippy::too_many_arguments)]
pub async fn serve_tcp_optional_openssl_with_bind_options_and_shutdown_deadline<Req, P, H>(
    server: Arc<NacelleServer<Req, P, H>>,
    addr: SocketAddr,
    tls_config: NacelleOpenSslConfig,
    bind_options: NacelleTcpBindOptions,
    detection_options: NacelleTlsDetectionOptions,
    shutdown: NacelleShutdownToken,
    drain_deadline: NacelleDrainDeadline,
) -> Result<(), NacelleError>
where
    Req: RequestMetadata + Send + 'static,
    P: Protocol<Req> + Send + Sync + 'static,
    H: Handler,
{
    let listener = bind_tcp_listener(addr, &bind_options)?;
    serve_tcp_optional_openssl_listener_with_options_and_shutdown_deadline(
        server,
        listener,
        tls_config,
        bind_options.stream,
        detection_options,
        shutdown,
        drain_deadline,
    )
    .await
}

#[cfg(unix)]
#[doc(hidden)]
pub async fn serve_unix_with_shutdown_deadline<Req, P, H>(
    server: Arc<NacelleServer<Req, P, H>>,
    path: impl AsRef<Path>,
    shutdown: NacelleShutdownToken,
    drain_deadline: NacelleDrainDeadline,
) -> Result<(), NacelleError>
where
    Req: RequestMetadata + Send + 'static,
    P: Protocol<Req> + Send + Sync + 'static,
    H: Handler,
{
    serve_unix_with_options_and_shutdown_deadline(
        server,
        path,
        NacelleUnixSocketOptions::default(),
        shutdown,
        drain_deadline,
    )
    .await
}

#[doc(hidden)]
pub async fn serve_tcp_with_options_and_shutdown_deadline<Req, P, H>(
    server: Arc<NacelleServer<Req, P, H>>,
    addr: SocketAddr,
    tcp_options: NacelleTcpOptions,
    shutdown: NacelleShutdownToken,
    drain_deadline: NacelleDrainDeadline,
) -> Result<(), NacelleError>
where
    Req: RequestMetadata + Send + 'static,
    P: Protocol<Req> + Send + Sync + 'static,
    H: Handler,
{
    let listener = tokio::net::TcpListener::bind(addr).await?;
    serve_tcp_listener_with_options_and_shutdown_deadline(
        server,
        listener,
        tcp_options,
        shutdown,
        drain_deadline,
    )
    .await
}

fn bind_tcp_listener(
    addr: SocketAddr,
    bind_options: &NacelleTcpBindOptions,
) -> std::io::Result<tokio::net::TcpListener> {
    let domain = if addr.is_ipv4() {
        socket2::Domain::IPV4
    } else {
        socket2::Domain::IPV6
    };
    let socket = socket2::Socket::new(domain, socket2::Type::STREAM, Some(socket2::Protocol::TCP))?;
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

#[cfg(unix)]
#[doc(hidden)]
pub async fn serve_unix_with_options_and_shutdown_deadline<Req, P, H>(
    server: Arc<NacelleServer<Req, P, H>>,
    path: impl AsRef<Path>,
    unix_options: NacelleUnixSocketOptions,
    shutdown: NacelleShutdownToken,
    drain_deadline: NacelleDrainDeadline,
) -> Result<(), NacelleError>
where
    Req: RequestMetadata + Send + 'static,
    P: Protocol<Req> + Send + Sync + 'static,
    H: Handler,
{
    let path = path.as_ref();
    unix_options.prepare_path(path)?;
    let listener = UnixListener::bind(path)?;
    unix_options.apply_to_path(path)?;
    serve_unix_listener_with_shutdown_deadline(
        server,
        listener,
        Some(path.to_path_buf()),
        shutdown,
        drain_deadline,
    )
    .await
}

#[cfg(feature = "openssl")]
#[doc(hidden)]
#[allow(clippy::too_many_arguments)]
pub async fn serve_tcp_optional_openssl_listener_with_options_and_shutdown_deadline<Req, P, H>(
    server: Arc<NacelleServer<Req, P, H>>,
    listener: tokio::net::TcpListener,
    tls_config: NacelleOpenSslConfig,
    tcp_options: NacelleTcpOptions,
    detection_options: NacelleTlsDetectionOptions,
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
                log_connection_result(joined, NacelleTransport::Tcp);
                continue;
            }
            accepted = listener.accept() => {
                let (stream, peer_addr) = accepted?;
                tcp_options.apply_to_stream(&stream)?;
                let connection = NacelleConnectionMeta::tcp(Some(peer_addr), local_addr);
                let connection_permit = match server.runtime_state().acquire_connection_for_peer(peer_addr.ip()) {
                    Ok(permit) => permit,
                    Err(error) => {
                        server
                            .telemetry()
                            .connection_rejected(NacelleTransport::Tcp, connection_rejection_reason(&error));
                        continue;
                    }
                };
                let server = server.clone();
                let acceptor = tls_config.acceptor();
                let detection_timeout = detection_options.timeout;
                connections.spawn(async move {
                    let _connection_permit = connection_permit;
                    let is_tls = match detect_tls_handshake(&stream, detection_timeout).await {
                        Ok(is_tls) => is_tls,
                        Err(error) => {
                            if matches!(error, NacelleError::Timeout(_)) {
                                server
                                    .telemetry()
                                    .timeout(NacelleTransport::Tcp, "tls_detect");
                            }
                            return Err(error);
                        }
                    };

                    if !is_tls {
                        return server.serve_io_without_connection_limit(stream, connection).await;
                    }

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
                                .timeout(NacelleTransport::Tcp, "tls_handshake");
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
        NacelleTransport::Tcp,
    );
    drain_connection_tasks(
        connections,
        drain_deadline.get(),
        NacelleTransport::Tcp,
        server.telemetry().clone(),
    )
    .await;
    Ok(())
}

#[doc(hidden)]
pub async fn serve_tcp_listener_with_shutdown_deadline<Req, P, H>(
    server: Arc<NacelleServer<Req, P, H>>,
    listener: tokio::net::TcpListener,
    shutdown: NacelleShutdownToken,
    drain_deadline: NacelleDrainDeadline,
) -> Result<(), NacelleError>
where
    Req: RequestMetadata + Send + 'static,
    P: Protocol<Req> + Send + Sync + 'static,
    H: Handler,
{
    serve_tcp_listener_with_options_and_shutdown_deadline(
        server,
        listener,
        NacelleTcpOptions::default(),
        shutdown,
        drain_deadline,
    )
    .await
}

#[doc(hidden)]
pub async fn serve_tcp_listener_with_options_and_shutdown_deadline<Req, P, H>(
    server: Arc<NacelleServer<Req, P, H>>,
    listener: tokio::net::TcpListener,
    tcp_options: NacelleTcpOptions,
    mut shutdown: NacelleShutdownToken,
    drain_deadline: NacelleDrainDeadline,
) -> Result<(), NacelleError>
where
    Req: RequestMetadata + Send + 'static,
    P: Protocol<Req> + Send + Sync + 'static,
    H: Handler,
{
    let mut connections = tokio::task::JoinSet::new();
    let local_addr = listener.local_addr().ok();
    loop {
        tokio::select! {
            biased;
            _ = shutdown.changed() => break,
            joined = connections.join_next(), if !connections.is_empty() => {
                log_connection_result(joined, NacelleTransport::Tcp);
                continue;
            }
            accepted = listener.accept() => {
                let (stream, peer_addr) = accepted?;
                tcp_options.apply_to_stream(&stream)?;
                let connection = NacelleConnectionMeta::tcp(Some(peer_addr), local_addr);
                let connection_permit = match server.runtime_state().acquire_connection_for_peer(peer_addr.ip()) {
                    Ok(permit) => permit,
                    Err(error) => {
                        server
                            .telemetry()
                            .connection_rejected(NacelleTransport::Tcp, connection_rejection_reason(&error));
                        continue;
                    }
                };
                let server = server.clone();
                connections.spawn(async move {
                    let _connection_permit = connection_permit;
                    server.serve_io_without_connection_limit(stream, connection).await
                });
            }
        }
    }
    server.telemetry().shutdown_event(
        NacelleTelemetryEventKind::ListenerStoppedAccepting,
        NacelleTransport::Tcp,
    );
    drain_connection_tasks(
        connections,
        drain_deadline.get(),
        NacelleTransport::Tcp,
        server.telemetry().clone(),
    )
    .await;
    Ok(())
}

#[cfg(unix)]
#[doc(hidden)]
pub async fn serve_unix_listener_with_shutdown_deadline<Req, P, H>(
    server: Arc<NacelleServer<Req, P, H>>,
    listener: UnixListener,
    local_path: Option<PathBuf>,
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
                log_connection_result(joined, NacelleTransport::UnixSocket);
                continue;
            }
            accepted = listener.accept() => {
                let (stream, _) = accepted?;
                let connection = NacelleConnectionMeta::unix_socket(local_path.clone());
                let connection_permit = match server.runtime_state().acquire_connection_tracked() {
                    Ok(permit) => permit,
                    Err(error) => {
                        server
                            .telemetry()
                            .connection_rejected(NacelleTransport::UnixSocket, connection_rejection_reason(&error));
                        continue;
                    }
                };
                let server = server.clone();
                connections.spawn(async move {
                    let _connection_permit = connection_permit;
                    server.serve_io_without_connection_limit(stream, connection).await
                });
            }
        }
    }
    server.telemetry().shutdown_event(
        NacelleTelemetryEventKind::ListenerStoppedAccepting,
        NacelleTransport::UnixSocket,
    );
    drain_connection_tasks(
        connections,
        drain_deadline.get(),
        NacelleTransport::UnixSocket,
        server.telemetry().clone(),
    )
    .await;
    Ok(())
}

#[cfg(feature = "rustls")]
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
                log_connection_result(joined, NacelleTransport::Tcp);
                continue;
            }
            accepted = listener.accept() => {
                let (stream, peer_addr) = accepted?;
                let _ = stream.set_nodelay(true);
                let connection = NacelleConnectionMeta::tcp(Some(peer_addr), local_addr);
                let connection_permit = match server.runtime_state().acquire_connection_for_peer(peer_addr.ip()) {
                    Ok(permit) => permit,
                    Err(error) => {
                        server
                            .telemetry()
                            .connection_rejected(NacelleTransport::Tcp, connection_rejection_reason(&error));
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
                                .timeout(NacelleTransport::Tcp, "tls_handshake");
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
        NacelleTransport::Tcp,
    );
    drain_connection_tasks(
        connections,
        drain_deadline.get(),
        NacelleTransport::Tcp,
        server.telemetry().clone(),
    )
    .await;
    Ok(())
}

#[cfg(feature = "openssl")]
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

#[cfg(feature = "openssl")]
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
                log_connection_result(joined, NacelleTransport::Tcp);
                continue;
            }
            accepted = listener.accept() => {
                let (stream, peer_addr) = accepted?;
                tcp_options.apply_to_stream(&stream)?;
                let connection = NacelleConnectionMeta::tcp(Some(peer_addr), local_addr);
                let connection_permit = match server.runtime_state().acquire_connection_for_peer(peer_addr.ip()) {
                    Ok(permit) => permit,
                    Err(error) => {
                        server
                            .telemetry()
                            .connection_rejected(NacelleTransport::Tcp, connection_rejection_reason(&error));
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
                                .timeout(NacelleTransport::Tcp, "tls_handshake");
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
        NacelleTransport::Tcp,
    );
    drain_connection_tasks(
        connections,
        drain_deadline.get(),
        NacelleTransport::Tcp,
        server.telemetry().clone(),
    )
    .await;
    Ok(())
}

#[cfg(feature = "openssl")]
fn openssl_tls_meta(ssl: &SslRef) -> NacelleConnectionTlsMeta {
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

#[cfg(feature = "openssl")]
async fn detect_tls_handshake(
    stream: &tokio::net::TcpStream,
    timeout: Duration,
) -> Result<bool, NacelleError> {
    let deadline = tokio::time::Instant::now() + timeout;
    let mut peek_buf = [0_u8; 3];
    loop {
        let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
        if remaining.is_zero() {
            return Err(NacelleError::Timeout("tls_detect"));
        }

        match tokio::time::timeout(remaining, stream.peek(&mut peek_buf)).await {
            Ok(Ok(0)) => return Err(NacelleError::ConnectionClosed),
            Ok(Ok(len)) if !peeked_bytes_can_be_tls(&peek_buf[..len]) => return Ok(false),
            Ok(Ok(len)) if len >= 3 => return Ok(true),
            Ok(Ok(_)) => {}
            Ok(Err(error)) => return Err(NacelleError::Io(error)),
            Err(_) => return Err(NacelleError::Timeout("tls_detect")),
        }

        tokio::time::sleep(Duration::from_millis(25)).await;
    }
}

#[cfg(feature = "openssl")]
fn peeked_bytes_can_be_tls(bytes: &[u8]) -> bool {
    match bytes {
        [] => true,
        [record_type] => *record_type == 0x16,
        [record_type, major] => *record_type == 0x16 && *major == 0x03,
        [record_type, major, minor, ..] => {
            *record_type == 0x16 && *major == 0x03 && (0x01..=0x04).contains(minor)
        }
    }
}

fn log_connection_result(
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

fn connection_rejection_reason(error: &NacelleError) -> &'static str {
    match error {
        NacelleError::ResourceLimit(reason) => reason,
        _ => "connections",
    }
}

async fn drain_connection_tasks(
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

#[cfg(all(test, feature = "tls-self-signed"))]
mod tests {
    use super::*;

    use bytes::{Bytes, BytesMut};
    use nacelle_core::handler::handler_fn;
    use nacelle_core::request::{NacelleRequest, RequestMetadata, TcpRequestMeta};
    use nacelle_core::response::NacelleResponse;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    use crate::protocol::{DecodedRequest, Protocol};
    use crate::server::TcpServer;

    #[derive(Debug)]
    struct TestRequest;

    impl RequestMetadata for TestRequest {
        fn opcode(&self) -> u64 {
            1
        }

        fn tcp_meta(&self, body_len: usize) -> TcpRequestMeta {
            TcpRequestMeta {
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
    async fn tcp_tls_self_signed_server_accepts_request() {
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
        let server = TcpServer::<TestRequest, ()>::builder()
            .protocol(TestProtocol)
            .handler(handler_fn(|_request: NacelleRequest| async move {
                Ok(NacelleResponse::tcp_bytes("ok"))
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

#[cfg(all(test, feature = "openssl"))]
mod openssl_tests {
    use super::*;

    use std::sync::atomic::{AtomicBool, Ordering};

    use bytes::{Bytes, BytesMut};
    use nacelle_core::handler::handler_fn;
    use nacelle_core::request::{NacelleRequest, RequestMetadata};
    use nacelle_core::response::NacelleResponse;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    use crate::protocol::{DecodedRequest, Protocol};
    use crate::server::TcpServer;

    #[derive(Debug)]
    struct PlainRequest;

    impl RequestMetadata for PlainRequest {
        fn opcode(&self) -> u64 {
            1
        }
    }

    struct PlainProtocol;

    impl Protocol<PlainRequest> for PlainProtocol {
        type ResponseContext = ();
        type ErrorContext = ();

        fn decode_head(
            &self,
            src: &mut BytesMut,
            _max_frame_len: usize,
        ) -> Result<Option<DecodedRequest<PlainRequest>>, NacelleError> {
            if src.is_empty() {
                return Ok(None);
            }
            src.clear();
            Ok(Some(DecodedRequest {
                request: PlainRequest,
                body_len: 0,
            }))
        }

        fn response_context(&self, _req: &PlainRequest) -> Self::ResponseContext {}

        fn error_context(&self, _req: &PlainRequest) -> Self::ErrorContext {}

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

    #[test]
    fn tls_detection_accepts_tls_handshake_prefix() {
        assert!(peeked_bytes_can_be_tls(&[0x16]));
        assert!(peeked_bytes_can_be_tls(&[0x16, 0x03]));
        assert!(peeked_bytes_can_be_tls(&[0x16, 0x03, 0x03]));
    }

    #[test]
    fn tls_detection_rejects_plain_protocol_prefix() {
        assert!(!peeked_bytes_can_be_tls(&[0x01]));
        assert!(!peeked_bytes_can_be_tls(&[0x16, 0x02]));
        assert!(!peeked_bytes_can_be_tls(&[0x16, 0x03, 0x05]));
    }

    #[tokio::test]
    async fn required_openssl_rejects_plaintext_before_handler() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("listener should bind");
        let addr = listener.local_addr().expect("listener should have addr");
        let (shutdown, token) = nacelle_core::lifecycle::NacelleShutdown::pair();
        let handler_called = Arc::new(AtomicBool::new(false));
        let server = TcpServer::<PlainRequest, ()>::builder()
            .protocol(PlainProtocol)
            .handler(handler_fn({
                let handler_called = handler_called.clone();
                move |_request: NacelleRequest| {
                    handler_called.store(true, Ordering::SeqCst);
                    async move { Ok(NacelleResponse::empty_tcp()) }
                }
            }))
            .build()
            .expect("server should build");
        let server_task = tokio::spawn(serve_tcp_openssl_listener_with_shutdown_deadline(
            Arc::new(server),
            listener,
            test_open_ssl_config(),
            token,
            NacelleDrainDeadline::new(Duration::from_millis(25)),
        ));

        let mut client = tokio::net::TcpStream::connect(addr)
            .await
            .expect("client should connect");
        client
            .write_all(&[0x01, 0x02, 0x03])
            .await
            .expect("plaintext should write");
        let mut response = [0_u8; 1];
        let _ = tokio::time::timeout(Duration::from_millis(500), client.read(&mut response)).await;

        assert!(!handler_called.load(Ordering::SeqCst));

        shutdown.shutdown();
        tokio::time::timeout(Duration::from_secs(1), server_task)
            .await
            .expect("server should stop")
            .expect("server task should join")
            .expect("server should exit");
    }

    fn test_open_ssl_config() -> NacelleOpenSslConfig {
        use openssl::asn1::Asn1Time;
        use openssl::bn::BigNum;
        use openssl::hash::MessageDigest;
        use openssl::pkey::PKey;
        use openssl::rsa::Rsa;
        use openssl::ssl::{SslAcceptor, SslMethod};
        use openssl::x509::{X509, X509NameBuilder};

        let rsa = Rsa::generate(2048).expect("rsa key should generate");
        let private_key = PKey::from_rsa(rsa).expect("pkey should build");
        let mut name = X509NameBuilder::new().expect("name should build");
        name.append_entry_by_text("CN", "localhost")
            .expect("common name should set");
        let name = name.build();
        let serial = BigNum::from_u32(1)
            .expect("serial should build")
            .to_asn1_integer()
            .expect("serial should convert");
        let mut certificate = X509::builder().expect("certificate should build");
        certificate.set_version(2).expect("version should set");
        certificate
            .set_serial_number(&serial)
            .expect("serial should set");
        certificate
            .set_subject_name(&name)
            .expect("subject should set");
        certificate
            .set_issuer_name(&name)
            .expect("issuer should set");
        certificate
            .set_pubkey(&private_key)
            .expect("public key should set");
        certificate
            .set_not_before(Asn1Time::days_from_now(0).expect("not before").as_ref())
            .expect("not before should set");
        certificate
            .set_not_after(Asn1Time::days_from_now(1).expect("not after").as_ref())
            .expect("not after should set");
        certificate
            .sign(&private_key, MessageDigest::sha256())
            .expect("certificate should sign");
        let certificate = certificate.build();

        let mut acceptor = SslAcceptor::mozilla_intermediate(SslMethod::tls_server())
            .expect("acceptor should build");
        acceptor
            .set_private_key(&private_key)
            .expect("private key should set");
        acceptor
            .set_certificate(&certificate)
            .expect("certificate should set");
        acceptor
            .check_private_key()
            .expect("private key should match");
        NacelleOpenSslConfig::from_acceptor(acceptor.build())
    }
}
