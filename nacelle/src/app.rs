#[cfg(feature = "raw_tcp")]
use std::net::SocketAddr;
#[cfg(unix)]
use std::path::Path;

use nacelle_core::config::NacelleConfig;
use nacelle_core::error::NacelleError;
use nacelle_core::handler::Handler;
use nacelle_core::lifecycle::NacelleShutdown;
use nacelle_core::limits::{NacelleLimits, NacelleRuntimeState};
use nacelle_core::telemetry::NacelleTelemetry;

use crate::host::NacelleHost;

#[cfg(all(feature = "raw_tcp", feature = "openssl"))]
use nacelle_core::tls::NacelleOpenSslConfig;
#[cfg(all(feature = "raw_tcp", feature = "openssl"))]
use nacelle_tcp::NacelleTlsDetectionOptions;
#[cfg(all(feature = "raw_tcp", unix))]
use nacelle_tcp::NacelleUnixSocketOptions;
#[cfg(feature = "raw_tcp")]
use nacelle_tcp::{NacelleTcpOptions, Protocol, RawTcpServer};

pub struct NacelleApp<H> {
    handler: H,
    config: NacelleConfig,
    telemetry: NacelleTelemetry,
    runtime_state: NacelleRuntimeState,
    shutdown: NacelleShutdown,
    drain_timeout: std::time::Duration,
}

impl<H> NacelleApp<H>
where
    H: Handler,
{
    pub fn new(handler: H) -> Self {
        Self {
            handler,
            config: NacelleConfig::default(),
            telemetry: NacelleTelemetry::default(),
            runtime_state: NacelleRuntimeState::default(),
            shutdown: NacelleShutdown::new(),
            drain_timeout: std::time::Duration::from_secs(30),
        }
    }

    pub fn with_config(mut self, config: NacelleConfig) -> Self {
        self.config = config;
        self
    }

    pub fn with_telemetry(mut self, telemetry: NacelleTelemetry) -> Self {
        self.telemetry = telemetry;
        self
    }

    pub fn with_limits(mut self, limits: NacelleLimits) -> Self {
        self.runtime_state = NacelleRuntimeState::new(limits);
        self
    }

    pub fn with_runtime_state(mut self, runtime_state: NacelleRuntimeState) -> Self {
        self.runtime_state = runtime_state;
        self
    }

    pub fn with_shutdown(mut self, shutdown: NacelleShutdown) -> Self {
        self.shutdown = shutdown;
        self
    }

    pub fn with_shutdown_drain_timeout(mut self, drain_timeout: std::time::Duration) -> Self {
        self.drain_timeout = drain_timeout;
        self
    }

    pub fn handler(&self) -> &H {
        &self.handler
    }
}

type ProtocolInstaller<H> =
    Box<dyn FnOnce(&mut NacelleHost, &NacelleApp<H>) -> Result<(), NacelleError> + Send>;

pub struct NacelleProtocols<H> {
    installers: Vec<ProtocolInstaller<H>>,
}

impl<H> Default for NacelleProtocols<H> {
    fn default() -> Self {
        Self::new()
    }
}

impl<H> NacelleProtocols<H> {
    pub fn new() -> Self {
        Self {
            installers: Vec::new(),
        }
    }
}

#[cfg(feature = "raw_tcp")]
impl<H> NacelleProtocols<H>
where
    H: Handler,
{
    pub fn raw_tcp<Req, P>(self, name: impl Into<String>, addr: SocketAddr, protocol: P) -> Self
    where
        Req: nacelle_core::request::RequestMetadata + Send + 'static,
        P: Protocol<Req> + Send + Sync + 'static,
    {
        self.raw_tcp_with_options(name, addr, protocol, NacelleTcpOptions::default())
    }

    pub fn raw_tcp_with_options<Req, P>(
        mut self,
        name: impl Into<String>,
        addr: SocketAddr,
        protocol: P,
        tcp_options: NacelleTcpOptions,
    ) -> Self
    where
        Req: nacelle_core::request::RequestMetadata + Send + 'static,
        P: Protocol<Req> + Send + Sync + 'static,
    {
        let name = name.into();
        self.installers.push(Box::new(move |host, app| {
            let server = raw_tcp_server::<Req, P, H>(protocol, app)?;
            host.enable_raw_tcp_with_options(name, addr, tcp_options, server);
            Ok(())
        }));
        self
    }

    #[cfg(all(feature = "raw_tcp", unix))]
    pub fn unix_socket<Req, P>(
        self,
        name: impl Into<String>,
        path: impl AsRef<Path>,
        protocol: P,
    ) -> Self
    where
        Req: nacelle_core::request::RequestMetadata + Send + 'static,
        P: Protocol<Req> + Send + Sync + 'static,
    {
        self.unix_socket_with_options(name, path, protocol, NacelleUnixSocketOptions::default())
    }

    #[cfg(all(feature = "raw_tcp", unix))]
    pub fn unix_socket_with_options<Req, P>(
        mut self,
        name: impl Into<String>,
        path: impl AsRef<Path>,
        protocol: P,
        unix_options: NacelleUnixSocketOptions,
    ) -> Self
    where
        Req: nacelle_core::request::RequestMetadata + Send + 'static,
        P: Protocol<Req> + Send + Sync + 'static,
    {
        let name = name.into();
        let path = path.as_ref().to_path_buf();
        self.installers.push(Box::new(move |host, app| {
            let server = raw_tcp_server::<Req, P, H>(protocol, app)?;
            host.enable_unix_socket_with_options(name, path, unix_options, server);
            Ok(())
        }));
        self
    }

    #[cfg(all(feature = "raw_tcp", feature = "openssl"))]
    pub fn raw_tcp_optional_openssl<Req, P>(
        self,
        name: impl Into<String>,
        addr: SocketAddr,
        protocol: P,
        tls_config: NacelleOpenSslConfig,
    ) -> Self
    where
        Req: nacelle_core::request::RequestMetadata + Send + 'static,
        P: Protocol<Req> + Send + Sync + 'static,
    {
        self.raw_tcp_optional_openssl_with_options(
            name,
            addr,
            protocol,
            tls_config,
            NacelleTcpOptions::default(),
            NacelleTlsDetectionOptions::default(),
        )
    }

    #[cfg(all(feature = "raw_tcp", feature = "openssl"))]
    pub fn raw_tcp_optional_openssl_with_options<Req, P>(
        mut self,
        name: impl Into<String>,
        addr: SocketAddr,
        protocol: P,
        tls_config: NacelleOpenSslConfig,
        tcp_options: NacelleTcpOptions,
        detection_options: NacelleTlsDetectionOptions,
    ) -> Self
    where
        Req: nacelle_core::request::RequestMetadata + Send + 'static,
        P: Protocol<Req> + Send + Sync + 'static,
    {
        let name = name.into();
        self.installers.push(Box::new(move |host, app| {
            let server = raw_tcp_server::<Req, P, H>(protocol, app)?;
            host.enable_raw_tcp_optional_openssl_with_options(
                name,
                addr,
                server,
                tls_config,
                tcp_options,
                detection_options,
            );
            Ok(())
        }));
        self
    }
}

pub async fn serve<H>(
    protocols: NacelleProtocols<H>,
    app: NacelleApp<H>,
) -> Result<(), NacelleError>
where
    H: Handler,
{
    let mut host = NacelleHost::new()
        .with_telemetry(app.telemetry.clone())
        .with_runtime_state(app.runtime_state.clone())
        .with_shutdown(app.shutdown.clone())
        .with_shutdown_drain_timeout(app.drain_timeout);
    for installer in protocols.installers {
        installer(&mut host, &app)?;
    }
    host.wait().await
}

#[cfg(feature = "raw_tcp")]
fn raw_tcp_server<Req, P, H>(
    protocol: P,
    app: &NacelleApp<H>,
) -> Result<RawTcpServer<Req, P, H>, NacelleError>
where
    Req: nacelle_core::request::RequestMetadata + Send + 'static,
    P: Protocol<Req> + Send + Sync + 'static,
    H: Handler,
{
    RawTcpServer::<Req, ()>::builder()
        .protocol(protocol)
        .config(app.config.clone())
        .telemetry(app.telemetry.clone())
        .runtime_state(app.runtime_state.clone())
        .handler(app.handler.clone())
        .build()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn protocols_start_empty() {
        let protocols = NacelleProtocols::<()>::new();

        assert_eq!(protocols.installers.len(), 0);
    }
}
