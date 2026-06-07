use std::error::Error as StdError;
use std::fmt::{Display, Formatter};
use std::io;

pub type BoxError = Box<dyn StdError + Send + Sync + 'static>;

#[derive(Debug)]
pub enum NacelleError {
    MissingProtocol,
    InvalidFrame(&'static str),
    FrameTooLarge { len: usize, max: usize },
    UnexpectedEof,
    ConnectionClosed,
    ResourceLimit(&'static str),
    Timeout(&'static str),
    Io(io::Error),
    Protocol(BoxError),
    Handler(BoxError),
    Join(crate::runtime::JoinError),
}

impl NacelleError {
    pub fn protocol(error: impl Into<BoxError>) -> Self {
        Self::Protocol(error.into())
    }

    pub fn handler(error: impl Into<BoxError>) -> Self {
        Self::Handler(error.into())
    }
}

impl Display for NacelleError {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::MissingProtocol => f.write_str("protocol is required"),
            Self::InvalidFrame(message) => write!(f, "invalid frame: {message}"),
            Self::FrameTooLarge { len, max } => {
                write!(f, "frame length {len} exceeds configured maximum {max}")
            }
            Self::UnexpectedEof => f.write_str("connection closed before the frame completed"),
            Self::ConnectionClosed => f.write_str("connection closed"),
            Self::ResourceLimit(name) => write!(f, "resource limit exceeded: {name}"),
            Self::Timeout(name) => write!(f, "operation timed out: {name}"),
            Self::Io(error) => write!(f, "i/o error: {error}"),
            Self::Protocol(error) => write!(f, "protocol error: {error}"),
            Self::Handler(error) => write!(f, "handler error: {error}"),
            Self::Join(error) => write!(f, "task join error: {error}"),
        }
    }
}

impl StdError for NacelleError {
    fn source(&self) -> Option<&(dyn StdError + 'static)> {
        match self {
            Self::Io(error) => Some(error),
            Self::Protocol(error) => Some(error.as_ref()),
            Self::Handler(error) => Some(error.as_ref()),
            Self::Join(error) => Some(error),
            _ => None,
        }
    }
}

impl From<io::Error> for NacelleError {
    fn from(value: io::Error) -> Self {
        Self::Io(value)
    }
}

impl From<crate::runtime::JoinError> for NacelleError {
    fn from(value: crate::runtime::JoinError) -> Self {
        Self::Join(value)
    }
}
