use std::marker::PhantomData;
use std::net::SocketAddr;
#[cfg(unix)]
use std::path::Path;
use std::sync::Arc;

use crate::connection::{serve_connection_with_connection_meta, serve_stream_with_connection_meta};
#[cfg(feature = "openssl")]
use crate::options::NacelleTlsDetectionOptions;
#[cfg(unix)]
use crate::options::NacelleUnixSocketOptions;
use crate::options::{NacelleTcpBindOptions, NacelleTcpOptions};
use crate::protocol::Protocol;
use nacelle_core::config::NacelleConfig;
use nacelle_core::error::NacelleError;
use nacelle_core::handler::Handler;
use nacelle_core::limits::NacelleRuntimeState;
use nacelle_core::request::{
    NacelleConnectionExtension, NacelleConnectionExtensionFactory, NacelleConnectionMeta,
    RequestMetadata,
};
use nacelle_core::telemetry::NacelleTelemetry;
#[cfg(feature = "openssl")]
use nacelle_core::tls::NacelleOpenSslConfig;
#[cfg(feature = "rustls")]
use nacelle_core::tls::NacelleTlsConfig;
use tokio::io::{AsyncRead, AsyncWrite};

pub struct Missing;
pub struct Present;

pub struct NacelleServer<Req, P, H = ()> {
    protocol: Arc<P>,
    handler: H,
    config: NacelleConfig,
    telemetry: NacelleTelemetry,
    runtime_state: NacelleRuntimeState,
    connection_extension_factory: Option<NacelleConnectionExtensionFactory>,
    _request: PhantomData<fn() -> Req>,
}

pub type TcpServer<Req, P, H = ()> = NacelleServer<Req, P, H>;

impl<Req, P, H> Clone for NacelleServer<Req, P, H>
where
    H: Clone,
{
    fn clone(&self) -> Self {
        Self {
            protocol: self.protocol.clone(),
            handler: self.handler.clone(),
            config: self.config.clone(),
            telemetry: self.telemetry.clone(),
            runtime_state: self.runtime_state.clone(),
            connection_extension_factory: self.connection_extension_factory.clone(),
            _request: PhantomData,
        }
    }
}

impl<Req> NacelleServer<Req, (), ()> {
    pub fn builder() -> NacelleServerBuilder<Req, Missing, Missing, (), ()> {
        NacelleServerBuilder {
            protocol: None,
            handler: None,
            config: NacelleConfig::default(),
            telemetry: NacelleTelemetry::default(),
            runtime_state: NacelleRuntimeState::default(),
            connection_extension_factory: None,
            _protocol: PhantomData,
            _handler: PhantomData,
            _request: PhantomData,
        }
    }
}

impl<Req, P, H> NacelleServer<Req, P, H>
where
    Req: RequestMetadata + Send + 'static,
    P: Protocol<Req> + Send + Sync + 'static,
    H: Handler,
{
    pub fn config(&self) -> &NacelleConfig {
        &self.config
    }

    pub fn runtime_state(&self) -> &NacelleRuntimeState {
        &self.runtime_state
    }

    pub fn telemetry(&self) -> &NacelleTelemetry {
        &self.telemetry
    }

    pub fn protocol(&self) -> &P {
        self.protocol.as_ref()
    }

    pub fn with_runtime_state(mut self, runtime_state: NacelleRuntimeState) -> Self {
        self.telemetry.register_runtime_state(runtime_state.clone());
        self.runtime_state = runtime_state;
        self
    }

    fn attach_connection_extension(
        &self,
        connection: NacelleConnectionMeta,
    ) -> NacelleConnectionMeta {
        let Some(factory) = &self.connection_extension_factory else {
            return connection;
        };
        let Some(extension) = factory(&connection) else {
            return connection;
        };
        connection.with_extension_arc(extension)
    }

    pub async fn serve_halves<R, W>(&self, reader: R, writer: W) -> Result<(), NacelleError>
    where
        R: AsyncRead + Unpin + Send + 'static,
        W: AsyncWrite + Unpin + Send + 'static,
    {
        serve_connection_with_connection_meta(
            reader,
            writer,
            self.protocol.clone(),
            self.handler.clone(),
            self.config.clone(),
            self.telemetry.clone(),
            self.runtime_state.clone(),
            self.attach_connection_extension(NacelleConnectionMeta::tcp(None, None)),
        )
        .await
    }

    /// Serve split I/O halves with caller-supplied connection metadata.
    pub async fn serve_halves_with_connection_meta<R, W>(
        &self,
        reader: R,
        writer: W,
        connection: NacelleConnectionMeta,
    ) -> Result<(), NacelleError>
    where
        R: AsyncRead + Unpin + Send + 'static,
        W: AsyncWrite + Unpin + Send + 'static,
    {
        serve_connection_with_connection_meta(
            reader,
            writer,
            self.protocol.clone(),
            self.handler.clone(),
            self.config.clone(),
            self.telemetry.clone(),
            self.runtime_state.clone(),
            self.attach_connection_extension(connection),
        )
        .await
    }

    /// Serve an I/O stream that implements Tokio's `AsyncRead + AsyncWrite`.
    pub async fn serve_io<IO>(&self, io: IO) -> Result<(), NacelleError>
    where
        IO: AsyncRead + AsyncWrite + Unpin + Send + 'static,
    {
        serve_stream_with_connection_meta(
            io,
            self.protocol.clone(),
            self.handler.clone(),
            self.config.clone(),
            self.telemetry.clone(),
            self.runtime_state.clone(),
            self.attach_connection_extension(NacelleConnectionMeta::tcp(None, None)),
        )
        .await
    }

    /// Serve an I/O stream with caller-supplied connection metadata.
    pub async fn serve_io_with_connection_meta<IO>(
        &self,
        io: IO,
        connection: NacelleConnectionMeta,
    ) -> Result<(), NacelleError>
    where
        IO: AsyncRead + AsyncWrite + Unpin + Send + 'static,
    {
        serve_stream_with_connection_meta(
            io,
            self.protocol.clone(),
            self.handler.clone(),
            self.config.clone(),
            self.telemetry.clone(),
            self.runtime_state.clone(),
            self.attach_connection_extension(connection),
        )
        .await
    }

    pub(crate) async fn serve_io_without_connection_limit<IO>(
        &self,
        io: IO,
        connection: NacelleConnectionMeta,
    ) -> Result<(), NacelleError>
    where
        IO: AsyncRead + AsyncWrite + Unpin + Send + 'static,
    {
        crate::connection::serve_stream_without_connection_limit_with_connection_meta(
            io,
            self.protocol.clone(),
            self.handler.clone(),
            self.config.clone(),
            self.telemetry.clone(),
            self.runtime_state.clone(),
            self.attach_connection_extension(connection),
        )
        .await
    }

    pub async fn serve_tcp(&self, addr: SocketAddr) -> Result<(), NacelleError> {
        crate::runtime::serve_tcp(Arc::<NacelleServer<Req, P, H>>::new(self.clone()), addr).await
    }

    pub async fn serve_tcp_with_shutdown(
        &self,
        addr: SocketAddr,
        shutdown: nacelle_core::lifecycle::NacelleShutdownToken,
    ) -> Result<(), NacelleError> {
        crate::runtime::serve_tcp_with_shutdown(
            Arc::<NacelleServer<Req, P, H>>::new(self.clone()),
            addr,
            shutdown,
        )
        .await
    }

    pub async fn serve_tcp_with_shutdown_timeout(
        &self,
        addr: SocketAddr,
        shutdown: nacelle_core::lifecycle::NacelleShutdownToken,
        drain_timeout: std::time::Duration,
    ) -> Result<(), NacelleError> {
        crate::runtime::serve_tcp_with_shutdown_timeout(
            Arc::<NacelleServer<Req, P, H>>::new(self.clone()),
            addr,
            shutdown,
            drain_timeout,
        )
        .await
    }

    pub async fn serve_tcp_with_options(
        &self,
        addr: SocketAddr,
        tcp_options: NacelleTcpOptions,
    ) -> Result<(), NacelleError> {
        crate::runtime::serve_tcp_with_options(
            Arc::<NacelleServer<Req, P, H>>::new(self.clone()),
            addr,
            tcp_options,
        )
        .await
    }

    pub async fn serve_tcp_with_options_and_shutdown(
        &self,
        addr: SocketAddr,
        tcp_options: NacelleTcpOptions,
        shutdown: nacelle_core::lifecycle::NacelleShutdownToken,
    ) -> Result<(), NacelleError> {
        crate::runtime::serve_tcp_with_options_and_shutdown(
            Arc::<NacelleServer<Req, P, H>>::new(self.clone()),
            addr,
            tcp_options,
            shutdown,
        )
        .await
    }

    pub async fn serve_tcp_with_options_and_shutdown_timeout(
        &self,
        addr: SocketAddr,
        tcp_options: NacelleTcpOptions,
        shutdown: nacelle_core::lifecycle::NacelleShutdownToken,
        drain_timeout: std::time::Duration,
    ) -> Result<(), NacelleError> {
        crate::runtime::serve_tcp_with_options_and_shutdown_timeout(
            Arc::<NacelleServer<Req, P, H>>::new(self.clone()),
            addr,
            tcp_options,
            shutdown,
            drain_timeout,
        )
        .await
    }

    #[doc(hidden)]
    pub async fn serve_tcp_with_bind_options_and_shutdown_deadline(
        &self,
        addr: SocketAddr,
        bind_options: NacelleTcpBindOptions,
        shutdown: nacelle_core::lifecycle::NacelleShutdownToken,
        drain_deadline: nacelle_core::lifecycle::NacelleDrainDeadline,
    ) -> Result<(), NacelleError> {
        crate::runtime::serve_tcp_with_bind_options_and_shutdown_deadline(
            Arc::<NacelleServer<Req, P, H>>::new(self.clone()),
            addr,
            bind_options,
            shutdown,
            drain_deadline,
        )
        .await
    }

    #[cfg(unix)]
    pub async fn serve_unix(&self, path: impl AsRef<Path>) -> Result<(), NacelleError> {
        crate::runtime::serve_unix(Arc::<NacelleServer<Req, P, H>>::new(self.clone()), path).await
    }

    #[cfg(unix)]
    pub async fn serve_unix_with_shutdown(
        &self,
        path: impl AsRef<Path>,
        shutdown: nacelle_core::lifecycle::NacelleShutdownToken,
    ) -> Result<(), NacelleError> {
        crate::runtime::serve_unix_with_shutdown(
            Arc::<NacelleServer<Req, P, H>>::new(self.clone()),
            path,
            shutdown,
        )
        .await
    }

    #[cfg(unix)]
    pub async fn serve_unix_with_shutdown_timeout(
        &self,
        path: impl AsRef<Path>,
        shutdown: nacelle_core::lifecycle::NacelleShutdownToken,
        drain_timeout: std::time::Duration,
    ) -> Result<(), NacelleError> {
        crate::runtime::serve_unix_with_shutdown_timeout(
            Arc::<NacelleServer<Req, P, H>>::new(self.clone()),
            path,
            shutdown,
            drain_timeout,
        )
        .await
    }

    #[cfg(unix)]
    pub async fn serve_unix_with_options(
        &self,
        path: impl AsRef<Path>,
        unix_options: NacelleUnixSocketOptions,
    ) -> Result<(), NacelleError> {
        crate::runtime::serve_unix_with_options(
            Arc::<NacelleServer<Req, P, H>>::new(self.clone()),
            path,
            unix_options,
        )
        .await
    }

    #[cfg(unix)]
    pub async fn serve_unix_with_options_and_shutdown(
        &self,
        path: impl AsRef<Path>,
        unix_options: NacelleUnixSocketOptions,
        shutdown: nacelle_core::lifecycle::NacelleShutdownToken,
    ) -> Result<(), NacelleError> {
        crate::runtime::serve_unix_with_options_and_shutdown(
            Arc::<NacelleServer<Req, P, H>>::new(self.clone()),
            path,
            unix_options,
            shutdown,
        )
        .await
    }

    #[cfg(unix)]
    pub async fn serve_unix_with_options_and_shutdown_timeout(
        &self,
        path: impl AsRef<Path>,
        unix_options: NacelleUnixSocketOptions,
        shutdown: nacelle_core::lifecycle::NacelleShutdownToken,
        drain_timeout: std::time::Duration,
    ) -> Result<(), NacelleError> {
        crate::runtime::serve_unix_with_options_and_shutdown_timeout(
            Arc::<NacelleServer<Req, P, H>>::new(self.clone()),
            path,
            unix_options,
            shutdown,
            drain_timeout,
        )
        .await
    }

    #[cfg(feature = "rustls")]
    pub async fn serve_tcp_tls(
        &self,
        addr: SocketAddr,
        tls_config: NacelleTlsConfig,
    ) -> Result<(), NacelleError> {
        crate::runtime::serve_tcp_tls(
            Arc::<NacelleServer<Req, P, H>>::new(self.clone()),
            addr,
            tls_config,
        )
        .await
    }

    #[cfg(feature = "rustls")]
    pub async fn serve_tcp_tls_with_shutdown(
        &self,
        addr: SocketAddr,
        tls_config: NacelleTlsConfig,
        shutdown: nacelle_core::lifecycle::NacelleShutdownToken,
    ) -> Result<(), NacelleError> {
        crate::runtime::serve_tcp_tls_with_shutdown(
            Arc::<NacelleServer<Req, P, H>>::new(self.clone()),
            addr,
            tls_config,
            shutdown,
        )
        .await
    }

    #[cfg(feature = "rustls")]
    pub async fn serve_tcp_tls_with_shutdown_timeout(
        &self,
        addr: SocketAddr,
        tls_config: NacelleTlsConfig,
        shutdown: nacelle_core::lifecycle::NacelleShutdownToken,
        drain_timeout: std::time::Duration,
    ) -> Result<(), NacelleError> {
        crate::runtime::serve_tcp_tls_with_shutdown_timeout(
            Arc::<NacelleServer<Req, P, H>>::new(self.clone()),
            addr,
            tls_config,
            shutdown,
            drain_timeout,
        )
        .await
    }

    #[cfg(feature = "openssl")]
    pub async fn serve_tcp_openssl(
        &self,
        addr: SocketAddr,
        tls_config: NacelleOpenSslConfig,
    ) -> Result<(), NacelleError> {
        crate::runtime::serve_tcp_openssl(
            Arc::<NacelleServer<Req, P, H>>::new(self.clone()),
            addr,
            tls_config,
        )
        .await
    }

    #[cfg(feature = "openssl")]
    pub async fn serve_tcp_openssl_with_shutdown(
        &self,
        addr: SocketAddr,
        tls_config: NacelleOpenSslConfig,
        shutdown: nacelle_core::lifecycle::NacelleShutdownToken,
    ) -> Result<(), NacelleError> {
        crate::runtime::serve_tcp_openssl_with_shutdown(
            Arc::<NacelleServer<Req, P, H>>::new(self.clone()),
            addr,
            tls_config,
            shutdown,
        )
        .await
    }

    #[cfg(feature = "openssl")]
    pub async fn serve_tcp_openssl_with_shutdown_timeout(
        &self,
        addr: SocketAddr,
        tls_config: NacelleOpenSslConfig,
        shutdown: nacelle_core::lifecycle::NacelleShutdownToken,
        drain_timeout: std::time::Duration,
    ) -> Result<(), NacelleError> {
        crate::runtime::serve_tcp_openssl_with_shutdown_timeout(
            Arc::<NacelleServer<Req, P, H>>::new(self.clone()),
            addr,
            tls_config,
            shutdown,
            drain_timeout,
        )
        .await
    }

    #[cfg(feature = "openssl")]
    pub async fn serve_tcp_openssl_with_options(
        &self,
        addr: SocketAddr,
        tls_config: NacelleOpenSslConfig,
        tcp_options: NacelleTcpOptions,
    ) -> Result<(), NacelleError> {
        crate::runtime::serve_tcp_openssl_with_options(
            Arc::<NacelleServer<Req, P, H>>::new(self.clone()),
            addr,
            tls_config,
            tcp_options,
        )
        .await
    }

    #[cfg(feature = "openssl")]
    pub async fn serve_tcp_openssl_with_options_and_shutdown(
        &self,
        addr: SocketAddr,
        tls_config: NacelleOpenSslConfig,
        tcp_options: NacelleTcpOptions,
        shutdown: nacelle_core::lifecycle::NacelleShutdownToken,
    ) -> Result<(), NacelleError> {
        crate::runtime::serve_tcp_openssl_with_options_and_shutdown(
            Arc::<NacelleServer<Req, P, H>>::new(self.clone()),
            addr,
            tls_config,
            tcp_options,
            shutdown,
        )
        .await
    }

    #[cfg(feature = "openssl")]
    pub async fn serve_tcp_openssl_with_options_and_shutdown_timeout(
        &self,
        addr: SocketAddr,
        tls_config: NacelleOpenSslConfig,
        tcp_options: NacelleTcpOptions,
        shutdown: nacelle_core::lifecycle::NacelleShutdownToken,
        drain_timeout: std::time::Duration,
    ) -> Result<(), NacelleError> {
        crate::runtime::serve_tcp_openssl_with_options_and_shutdown_timeout(
            Arc::<NacelleServer<Req, P, H>>::new(self.clone()),
            addr,
            tls_config,
            tcp_options,
            shutdown,
            drain_timeout,
        )
        .await
    }

    #[cfg(feature = "openssl")]
    #[doc(hidden)]
    pub async fn serve_tcp_openssl_with_bind_options_and_shutdown_deadline(
        &self,
        addr: SocketAddr,
        tls_config: NacelleOpenSslConfig,
        bind_options: NacelleTcpBindOptions,
        shutdown: nacelle_core::lifecycle::NacelleShutdownToken,
        drain_deadline: nacelle_core::lifecycle::NacelleDrainDeadline,
    ) -> Result<(), NacelleError> {
        crate::runtime::serve_tcp_openssl_with_bind_options_and_shutdown_deadline(
            Arc::<NacelleServer<Req, P, H>>::new(self.clone()),
            addr,
            tls_config,
            bind_options,
            shutdown,
            drain_deadline,
        )
        .await
    }

    #[cfg(feature = "openssl")]
    pub async fn serve_tcp_optional_openssl(
        &self,
        addr: SocketAddr,
        tls_config: NacelleOpenSslConfig,
    ) -> Result<(), NacelleError> {
        crate::runtime::serve_tcp_optional_openssl(
            Arc::<NacelleServer<Req, P, H>>::new(self.clone()),
            addr,
            tls_config,
        )
        .await
    }

    #[cfg(feature = "openssl")]
    pub async fn serve_tcp_optional_openssl_with_shutdown(
        &self,
        addr: SocketAddr,
        tls_config: NacelleOpenSslConfig,
        shutdown: nacelle_core::lifecycle::NacelleShutdownToken,
    ) -> Result<(), NacelleError> {
        crate::runtime::serve_tcp_optional_openssl_with_shutdown(
            Arc::<NacelleServer<Req, P, H>>::new(self.clone()),
            addr,
            tls_config,
            shutdown,
        )
        .await
    }

    #[cfg(feature = "openssl")]
    pub async fn serve_tcp_optional_openssl_with_options(
        &self,
        addr: SocketAddr,
        tls_config: NacelleOpenSslConfig,
        tcp_options: NacelleTcpOptions,
        detection_options: NacelleTlsDetectionOptions,
    ) -> Result<(), NacelleError> {
        crate::runtime::serve_tcp_optional_openssl_with_options(
            Arc::<NacelleServer<Req, P, H>>::new(self.clone()),
            addr,
            tls_config,
            tcp_options,
            detection_options,
        )
        .await
    }

    #[cfg(feature = "openssl")]
    pub async fn serve_tcp_optional_openssl_with_options_and_shutdown_timeout(
        &self,
        addr: SocketAddr,
        tls_config: NacelleOpenSslConfig,
        tcp_options: NacelleTcpOptions,
        detection_options: NacelleTlsDetectionOptions,
        shutdown: nacelle_core::lifecycle::NacelleShutdownToken,
        drain_timeout: std::time::Duration,
    ) -> Result<(), NacelleError> {
        crate::runtime::serve_tcp_optional_openssl_with_options_and_shutdown_timeout(
            Arc::<NacelleServer<Req, P, H>>::new(self.clone()),
            addr,
            tls_config,
            tcp_options,
            detection_options,
            shutdown,
            drain_timeout,
        )
        .await
    }

    #[cfg(feature = "openssl")]
    #[doc(hidden)]
    #[allow(clippy::too_many_arguments)]
    pub async fn serve_tcp_optional_openssl_with_bind_options_and_shutdown_deadline(
        &self,
        addr: SocketAddr,
        tls_config: NacelleOpenSslConfig,
        bind_options: NacelleTcpBindOptions,
        detection_options: NacelleTlsDetectionOptions,
        shutdown: nacelle_core::lifecycle::NacelleShutdownToken,
        drain_deadline: nacelle_core::lifecycle::NacelleDrainDeadline,
    ) -> Result<(), NacelleError> {
        crate::runtime::serve_tcp_optional_openssl_with_bind_options_and_shutdown_deadline(
            Arc::<NacelleServer<Req, P, H>>::new(self.clone()),
            addr,
            tls_config,
            bind_options,
            detection_options,
            shutdown,
            drain_deadline,
        )
        .await
    }
}

pub struct NacelleServerBuilder<Req, ProtocolState, HandlerState, P, H> {
    protocol: Option<Arc<P>>,
    handler: Option<H>,
    config: NacelleConfig,
    telemetry: NacelleTelemetry,
    runtime_state: NacelleRuntimeState,
    connection_extension_factory: Option<NacelleConnectionExtensionFactory>,
    _protocol: PhantomData<ProtocolState>,
    _handler: PhantomData<HandlerState>,
    _request: PhantomData<fn() -> Req>,
}

impl<Req, ProtocolState, HandlerState, P, H>
    NacelleServerBuilder<Req, ProtocolState, HandlerState, P, H>
{
    pub fn config(mut self, config: NacelleConfig) -> Self {
        self.config = config;
        self
    }

    pub fn telemetry(mut self, telemetry: NacelleTelemetry) -> Self {
        self.telemetry = telemetry;
        self
    }

    pub fn runtime_state(mut self, runtime_state: NacelleRuntimeState) -> Self {
        self.runtime_state = runtime_state;
        self
    }

    pub fn connection_extension_factory<F, E>(mut self, factory: F) -> Self
    where
        F: Fn(&NacelleConnectionMeta) -> E + Send + Sync + 'static,
        E: Send + Sync + 'static,
    {
        self.connection_extension_factory = Some(Arc::new(move |meta| {
            Some(Arc::new(factory(meta)) as NacelleConnectionExtension)
        }));
        self
    }

    pub fn optional_connection_extension_factory<F, E>(mut self, factory: F) -> Self
    where
        F: Fn(&NacelleConnectionMeta) -> Option<E> + Send + Sync + 'static,
        E: Send + Sync + 'static,
    {
        self.connection_extension_factory = Some(Arc::new(move |meta| {
            factory(meta).map(|extension| Arc::new(extension) as NacelleConnectionExtension)
        }));
        self
    }

    pub fn connection_extension_factory_arc(
        mut self,
        factory: NacelleConnectionExtensionFactory,
    ) -> Self {
        self.connection_extension_factory = Some(factory);
        self
    }
}

impl<Req, HandlerState, P, H> NacelleServerBuilder<Req, Missing, HandlerState, P, H> {
    pub fn protocol<P2>(
        self,
        protocol: P2,
    ) -> NacelleServerBuilder<Req, Present, HandlerState, P2, H> {
        NacelleServerBuilder {
            protocol: Some(Arc::new(protocol)),
            handler: self.handler,
            config: self.config,
            telemetry: self.telemetry,
            runtime_state: self.runtime_state,
            connection_extension_factory: self.connection_extension_factory,
            _protocol: PhantomData,
            _handler: PhantomData,
            _request: PhantomData,
        }
    }
}

impl<Req, ProtocolState, P, H> NacelleServerBuilder<Req, ProtocolState, Missing, P, H> {
    pub fn handler<H2>(
        self,
        handler: H2,
    ) -> NacelleServerBuilder<Req, ProtocolState, Present, P, H2> {
        NacelleServerBuilder {
            protocol: self.protocol,
            handler: Some(handler),
            config: self.config,
            telemetry: self.telemetry,
            runtime_state: self.runtime_state,
            connection_extension_factory: self.connection_extension_factory,
            _protocol: PhantomData,
            _handler: PhantomData,
            _request: PhantomData,
        }
    }
}

impl<Req, P, H> NacelleServerBuilder<Req, Present, Present, P, H>
where
    Req: RequestMetadata + Send + 'static,
    P: Protocol<Req> + Send + Sync + 'static,
    H: Handler,
{
    pub fn build(self) -> Result<NacelleServer<Req, P, H>, NacelleError> {
        let protocol = self.protocol.ok_or(NacelleError::MissingProtocol)?;
        let handler = self.handler.expect("handler state guarantees a handler");

        self.telemetry
            .register_runtime_state(self.runtime_state.clone());

        Ok(NacelleServer {
            protocol,
            handler,
            config: self.config,
            telemetry: self.telemetry,
            runtime_state: self.runtime_state,
            connection_extension_factory: self.connection_extension_factory,
            _request: PhantomData,
        })
    }
}
