#[cfg(any(feature = "raw_tcp", feature = "http"))]
use std::net::SocketAddr;

use tokio::task::JoinSet;

use crate::error::NacelleError;
use crate::limits::{NacelleLimits, NacelleRuntimeState};
use crate::telemetry::NacelleTelemetry;
#[cfg(any(feature = "raw_tcp", feature = "http"))]
use crate::telemetry::NacelleTransport;

pub struct NacelleHost {
    telemetry: NacelleTelemetry,
    runtime_state: NacelleRuntimeState,
    tasks: JoinSet<Result<(), NacelleError>>,
}

impl Default for NacelleHost {
    fn default() -> Self {
        Self::new()
    }
}

impl NacelleHost {
    pub fn new() -> Self {
        Self {
            telemetry: NacelleTelemetry::default(),
            runtime_state: NacelleRuntimeState::default(),
            tasks: JoinSet::new(),
        }
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

    #[cfg(feature = "raw_tcp")]
    pub fn enable_raw_tcp<Req, P, H>(
        &mut self,
        name: impl Into<String>,
        addr: SocketAddr,
        server: crate::server::RawTcpServer<Req, P, H>,
    ) -> &mut Self
    where
        Req: crate::request::RequestMetadata + Send + 'static,
        P: crate::protocol::Protocol<Req> + Send + Sync + 'static,
        H: crate::handler::Handler,
    {
        let name = name.into();
        let telemetry = self.telemetry.clone();
        let server = server.with_runtime_state(self.runtime_state.clone());
        telemetry.listener_configured(NacelleTransport::RawTcp, &name, &addr.to_string());
        self.tasks.spawn(async move {
            let result = server.serve_tcp(addr).await;
            if let Err(error) = &result {
                telemetry.listener_failed(
                    NacelleTransport::RawTcp,
                    &name,
                    &addr.to_string(),
                    error,
                );
            }
            result
        });
        self
    }

    #[cfg(feature = "http")]
    pub fn enable_http<H>(
        &mut self,
        name: impl Into<String>,
        addr: SocketAddr,
        server: crate::http_server::HyperServer<H>,
    ) -> &mut Self
    where
        H: crate::handler::Handler,
    {
        let name = name.into();
        let telemetry = self.telemetry.clone();
        let server = server.with_runtime_state(self.runtime_state.clone());
        telemetry.listener_configured(NacelleTransport::Http, &name, &addr.to_string());
        self.tasks.spawn(async move {
            let result = server.serve(addr).await;
            if let Err(error) = &result {
                telemetry.listener_failed(NacelleTransport::Http, &name, &addr.to_string(), error);
            }
            result
        });
        self
    }

    pub async fn wait(mut self) -> Result<(), NacelleError> {
        while let Some(result) = self.tasks.join_next().await {
            result??;
        }
        Ok(())
    }
}
