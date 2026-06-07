//! Shared primitives for Nacelle transports.

pub mod config;
pub mod error;
pub mod handler;
pub mod lifecycle;
pub mod limits;
pub mod request;
pub mod response;
pub mod runtime;
pub mod telemetry;
#[cfg(feature = "tls")]
pub mod tls;
#[cfg(feature = "tower")]
pub mod tower;

pub use config::{NacelleConfig, RequestBodyMode};
pub use error::{BoxError, NacelleError};
pub use handler::{Handler, HandlerFn, handler_fn};
pub use lifecycle::{NacelleShutdown, NacelleShutdownToken};
pub use limits::{MemoryReservation, NacelleLimits, NacelleRuntimeState, TrackedPermit};
#[cfg(feature = "http-types")]
pub use request::HttpRequestMeta;
pub use request::{
    NacelleBody, NacelleRequest, NacelleRequestMeta, RawTcpRequestMeta, RequestMetadata,
};
#[cfg(feature = "http-types")]
pub use response::HttpResponseMeta;
pub use response::{NacelleResponse, NacelleResponseMeta, RawTcpResponseMeta};
pub use telemetry::{
    NacelleInMemoryTelemetrySink, NacelleTelemetry, NacelleTelemetryEvent,
    NacelleTelemetryEventKind, NacelleTelemetrySink, NacelleTransport,
};
#[cfg(feature = "tls-self-signed")]
pub use tls::NacelleGeneratedTlsConfig;
#[cfg(feature = "tls")]
pub use tls::{NacelleTlsConfig, NacelleTlsProvider};
#[cfg(feature = "tower")]
pub use tower::handler_from_tower_service;
