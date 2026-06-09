//! TCP transport for Nacelle.

pub mod connection;
pub mod options;
pub mod protocol;
pub mod runtime;
pub mod server;

pub use connection::{serve_connection, serve_stream};
#[cfg(unix)]
pub use options::NacelleUnixSocketOptions;
pub use options::{NacelleTcpKeepalive, NacelleTcpOptions, NacelleTlsDetectionOptions};
pub use protocol::{DecodedRequest, Protocol};
pub use server::{NacelleServer, NacelleServerBuilder, TcpServer};
