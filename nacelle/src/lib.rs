//! Streaming application handlers across raw TCP and HTTP transports.
//!
//! Nacelle centers application code around one async handler shape:
//!
//! ```rust,no_run
//! # use nacelle::{NacelleError, NacelleRequest, NacelleResponse};
//! async fn handle(request: NacelleRequest) -> Result<NacelleResponse, NacelleError> {
//!     Ok(NacelleResponse::raw_tcp(request.body))
//! }
//! ```
//!
//! Use [`handler_fn`] for simple services, [`RawTcpServer`] for custom raw TCP
//! protocols, [`HyperServer`] for HTTP/1, and [`NacelleHost`] when one process
//! owns several listeners with shared limits.
//!
//! Production deployments should configure [`NacelleLimits`] explicitly and
//! attach [`NacelleTelemetry`] to expose low-cardinality lifecycle, request,
//! rejection, timeout, and byte-accounting events.
//!
//! Additional operational notes live in the repository `docs/` directory.

pub mod config;
#[cfg(feature = "raw_tcp")]
pub mod connection;
pub mod error;
pub mod handler;
pub mod host;
#[cfg(feature = "http")]
pub mod http_server;
pub mod lifecycle;
pub mod limits;
#[cfg(feature = "raw_tcp")]
pub mod protocol;
#[cfg(feature = "reference_protocol")]
pub mod reference_protocol;
pub mod request;
pub mod response;
pub mod runtime;
#[cfg(feature = "raw_tcp")]
pub mod server;
pub mod telemetry;
#[cfg(feature = "tower")]
pub mod tower;
#[cfg(feature = "reference_protocol")]
pub mod util;

pub use config::{NacelleConfig, RequestBodyMode};
#[cfg(feature = "raw_tcp")]
pub use connection::{serve_connection, serve_stream};
pub use error::{BoxError, NacelleError};
pub use handler::{Handler, HandlerFn, handler_fn};
pub use host::NacelleHost;
#[cfg(feature = "http")]
pub use http_server::HyperServer;
pub use lifecycle::{NacelleShutdown, NacelleShutdownToken};
pub use limits::{MemoryReservation, NacelleLimits, NacelleRuntimeState};
#[cfg(feature = "raw_tcp")]
pub use protocol::{DecodedRequest, Protocol};
#[cfg(feature = "reference_protocol")]
pub use reference_protocol::{
    FRAME_FLAG_END, FRAME_FLAG_ERROR, FRAME_FLAG_START, FrameRequest, LengthDelimitedProtocol,
};
#[cfg(feature = "http")]
pub use request::HttpRequestMeta;
pub use request::{
    NacelleBody, NacelleRequest, NacelleRequestMeta, RawTcpRequestMeta, RequestMetadata,
};
#[cfg(feature = "http")]
pub use response::HttpResponseMeta;
pub use response::{NacelleResponse, NacelleResponseMeta, RawTcpResponseMeta};
#[cfg(feature = "raw_tcp")]
pub use server::{NacelleServer, NacelleServerBuilder, RawTcpServer};
pub use telemetry::{
    NacelleInMemoryTelemetrySink, NacelleTelemetry, NacelleTelemetryEvent,
    NacelleTelemetryEventKind, NacelleTelemetrySink, NacelleTransport,
};
#[cfg(feature = "tower")]
pub use tower::handler_from_tower_service;
