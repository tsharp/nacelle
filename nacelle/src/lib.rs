pub mod config;
#[cfg(feature = "raw_tcp")]
pub mod connection;
pub mod error;
#[cfg(feature = "raw_tcp")]
pub mod frame;
pub mod handler;
#[cfg(feature = "http")]
pub mod http_server;
#[cfg(feature = "raw_tcp")]
pub mod protocol;
pub mod request;
pub mod response;
pub mod runtime;
#[cfg(feature = "raw_tcp")]
pub mod server;
#[cfg(feature = "raw_tcp")]
pub mod util;

pub use config::NacelleConfig;
#[cfg(feature = "raw_tcp")]
pub use connection::serve_connection;
pub use error::{BoxError, NacelleError};
#[cfg(feature = "raw_tcp")]
pub use frame::{
    FRAME_FLAG_END, FRAME_FLAG_ERROR, FRAME_FLAG_START, FrameRequest, LengthDelimitedProtocol,
};
pub use handler::{BoxedHandler, Handler, handler_fn, handler_from_trait};
#[cfg(feature = "http")]
pub use http_server::HyperServer;
#[cfg(feature = "raw_tcp")]
pub use protocol::{DecodedRequest, Protocol};
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
