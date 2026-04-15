pub mod config;
pub mod connection;
pub mod error;
pub mod frame;
pub mod handler;
pub mod metrics;
pub mod protocol;
pub mod registry;
pub mod request;
pub mod runtime;
pub mod server;
pub mod util;

pub use config::NacelleConfig;
pub use connection::serve_connection;
pub use error::{BoxError, NacelleError};
pub use frame::{
    FRAME_FLAG_END, FRAME_FLAG_ERROR, FRAME_FLAG_START, FrameRequest, LengthDelimitedProtocol,
};
pub use handler::{BoxedHandler, Handler, handler_fn, handler_from_trait};
pub use protocol::{DecodedRequest, Protocol};
pub use registry::{HandlerRegistry, RegistryStrategy};
pub use request::{RequestBody, RequestMetadata, ResponseWriter};
pub use server::{NacelleServer, NacelleServerBuilder};
