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

pub use nacelle_core::{config, error, handler, lifecycle, limits, request, response, telemetry};
#[cfg(feature = "raw_tcp")]
pub use nacelle_tcp::{connection, protocol, server};

pub mod host;
#[cfg(feature = "http")]
pub use nacelle_http::server as http_server;
#[cfg(feature = "reference_protocol")]
pub mod reference_protocol;
pub mod runtime {
    pub use nacelle_core::runtime::*;
    #[cfg(feature = "raw_tcp")]
    pub use nacelle_tcp::runtime::{
        serve_tcp, serve_tcp_listener_with_options_and_shutdown_deadline,
        serve_tcp_listener_with_shutdown_deadline, serve_tcp_with_options,
        serve_tcp_with_options_and_shutdown, serve_tcp_with_options_and_shutdown_deadline,
        serve_tcp_with_options_and_shutdown_timeout, serve_tcp_with_shutdown,
        serve_tcp_with_shutdown_deadline, serve_tcp_with_shutdown_timeout,
    };
    #[cfg(all(feature = "raw_tcp", feature = "openssl"))]
    pub use nacelle_tcp::runtime::{
        serve_tcp_openssl, serve_tcp_openssl_listener_with_shutdown_deadline,
        serve_tcp_openssl_with_shutdown, serve_tcp_openssl_with_shutdown_deadline,
        serve_tcp_openssl_with_shutdown_timeout,
    };
    #[cfg(all(feature = "raw_tcp", feature = "rustls"))]
    pub use nacelle_tcp::runtime::{
        serve_tcp_tls, serve_tcp_tls_listener_with_shutdown_deadline, serve_tcp_tls_with_shutdown,
        serve_tcp_tls_with_shutdown_deadline, serve_tcp_tls_with_shutdown_timeout,
    };
    #[cfg(all(feature = "raw_tcp", unix))]
    pub use nacelle_tcp::runtime::{
        serve_unix, serve_unix_listener_with_shutdown_deadline, serve_unix_with_options,
        serve_unix_with_options_and_shutdown, serve_unix_with_options_and_shutdown_deadline,
        serve_unix_with_options_and_shutdown_timeout, serve_unix_with_shutdown,
        serve_unix_with_shutdown_deadline, serve_unix_with_shutdown_timeout,
    };
}
#[cfg(any(feature = "tls", feature = "openssl"))]
pub use nacelle_core::tls;
#[cfg(feature = "tower")]
pub use nacelle_core::tower;
#[cfg(feature = "reference_protocol")]
pub mod util;

pub use host::NacelleHost;
#[cfg(feature = "tls-self-signed")]
pub use nacelle_core::NacelleGeneratedTlsConfig;
#[cfg(feature = "openssl")]
pub use nacelle_core::NacelleOpenSslConfig;
#[cfg(feature = "rustls")]
pub use nacelle_core::NacelleTlsConfig;
#[cfg(any(feature = "tls", feature = "openssl"))]
pub use nacelle_core::NacelleTlsProvider;
#[cfg(feature = "tower")]
pub use nacelle_core::handler_from_tower_service;
pub use nacelle_core::{
    BoxError, Handler, HandlerFn, MemoryReservation, NacelleBody, NacelleConfig,
    NacelleConnectionExtension, NacelleConnectionExtensionFactory, NacelleConnectionMeta,
    NacelleConnectionTlsMeta, NacelleError, NacelleInMemoryTelemetrySink, NacelleLimits,
    NacelleRequest, NacelleRequestMeta, NacelleResponse, NacelleResponseMeta, NacelleRuntimeState,
    NacelleShutdown, NacelleShutdownToken, NacelleTelemetry, NacelleTelemetryEvent,
    NacelleTelemetryEventKind, NacelleTelemetrySink, NacelleTransport, RawTcpRequestMeta,
    RawTcpResponseMeta, RequestBodyMode, RequestMetadata, TrackedPermit, handler_fn,
};
#[cfg(feature = "http")]
pub use nacelle_core::{HttpRequestMeta, HttpResponseMeta};
#[cfg(feature = "http")]
pub use nacelle_http::{HyperServer, NacelleHttpPolicy};
#[cfg(all(feature = "raw_tcp", unix))]
pub use nacelle_tcp::NacelleUnixSocketOptions;
#[cfg(feature = "raw_tcp")]
pub use nacelle_tcp::{
    DecodedRequest, NacelleServer, NacelleServerBuilder, NacelleTcpKeepalive, NacelleTcpOptions,
    Protocol, RawTcpServer, serve_connection, serve_stream,
};
#[cfg(feature = "reference_protocol")]
pub use reference_protocol::{
    FRAME_FLAG_END, FRAME_FLAG_ERROR, FRAME_FLAG_START, FrameRequest, LengthDelimitedProtocol,
};
