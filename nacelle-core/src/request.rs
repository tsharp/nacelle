use std::any::Any;
use std::fmt;
use std::net::{IpAddr, SocketAddr};
use std::pin::Pin;
use std::sync::Arc;
use std::task::{Context, Poll};

use bytes::Bytes;
use futures_core::Stream;
use tokio::sync::mpsc;

use crate::error::NacelleError;
use crate::limits::MemoryReservation;
use crate::telemetry::NacelleTransport;

pub type NacelleConnectionExtension = Arc<dyn Any + Send + Sync>;
pub type NacelleConnectionExtensionFactory =
    Arc<dyn Fn(&NacelleConnectionMeta) -> Option<NacelleConnectionExtension> + Send + Sync>;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NacelleConnectionTlsMeta {
    pub provider: &'static str,
    pub protocol: Option<String>,
    pub cipher_suite: Option<String>,
    pub server_name: Option<String>,
}

impl NacelleConnectionTlsMeta {
    pub fn new(provider: &'static str) -> Self {
        Self {
            provider,
            protocol: None,
            cipher_suite: None,
            server_name: None,
        }
    }

    pub fn with_protocol(mut self, protocol: impl Into<String>) -> Self {
        self.protocol = Some(protocol.into());
        self
    }

    pub fn with_cipher_suite(mut self, cipher_suite: impl Into<String>) -> Self {
        self.cipher_suite = Some(cipher_suite.into());
        self
    }

    pub fn with_server_name(mut self, server_name: impl Into<String>) -> Self {
        self.server_name = Some(server_name.into());
        self
    }
}

#[derive(Clone)]
pub struct NacelleConnectionMeta {
    pub transport: NacelleTransport,
    pub peer_addr: Option<SocketAddr>,
    pub peer_ip: Option<IpAddr>,
    pub local_addr: Option<SocketAddr>,
    pub tls: Option<NacelleConnectionTlsMeta>,
    extension: Option<NacelleConnectionExtension>,
}

impl fmt::Debug for NacelleConnectionMeta {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("NacelleConnectionMeta")
            .field("transport", &self.transport)
            .field("peer_addr", &self.peer_addr)
            .field("peer_ip", &self.peer_ip)
            .field("local_addr", &self.local_addr)
            .field("tls", &self.tls)
            .field("extension", &self.extension.as_ref().map(|_| "<extension>"))
            .finish()
    }
}

impl NacelleConnectionMeta {
    pub fn raw_tcp(peer_addr: Option<SocketAddr>, local_addr: Option<SocketAddr>) -> Self {
        Self {
            transport: NacelleTransport::RawTcp,
            peer_ip: peer_addr.map(|addr| addr.ip()),
            peer_addr,
            local_addr,
            tls: None,
            extension: None,
        }
    }

    pub fn http(peer_ip: Option<IpAddr>) -> Self {
        Self {
            transport: NacelleTransport::Http,
            peer_addr: None,
            peer_ip,
            local_addr: None,
            tls: None,
            extension: None,
        }
    }

    pub fn with_tls(mut self, tls: NacelleConnectionTlsMeta) -> Self {
        self.tls = Some(tls);
        self
    }

    pub fn with_extension<T>(self, extension: T) -> Self
    where
        T: Any + Send + Sync + 'static,
    {
        self.with_extension_arc(Arc::new(extension))
    }

    pub fn with_extension_arc(mut self, extension: NacelleConnectionExtension) -> Self {
        self.extension = Some(extension);
        self
    }

    pub fn extension<T>(&self) -> Option<Arc<T>>
    where
        T: Any + Send + Sync + 'static,
    {
        self.extension.clone()?.downcast::<T>().ok()
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RawTcpRequestMeta {
    pub request_id: Option<u64>,
    pub opcode: u64,
    pub flags: u32,
    pub body_len: usize,
}

#[cfg(feature = "http-types")]
#[derive(Debug, Clone)]
pub struct HttpRequestMeta {
    pub method: http::Method,
    pub uri: http::Uri,
    pub headers: http::HeaderMap,
    pub peer_ip: Option<IpAddr>,
}

#[derive(Debug, Clone)]
pub enum NacelleRequestMeta {
    RawTcp(RawTcpRequestMeta),
    #[cfg(feature = "http-types")]
    Http(HttpRequestMeta),
}

pub trait RequestMetadata: Send + 'static {
    fn opcode(&self) -> u64;

    fn raw_tcp_meta(&self, body_len: usize) -> RawTcpRequestMeta {
        RawTcpRequestMeta {
            request_id: None,
            opcode: self.opcode(),
            flags: 0,
            body_len,
        }
    }
}

pub struct NacelleRequest {
    pub connection: NacelleConnectionMeta,
    pub meta: NacelleRequestMeta,
    pub body: NacelleBody,
}

impl NacelleRequest {
    pub fn raw_tcp_meta(&self) -> Option<&RawTcpRequestMeta> {
        match &self.meta {
            NacelleRequestMeta::RawTcp(meta) => Some(meta),
            #[cfg(feature = "http-types")]
            NacelleRequestMeta::Http(_) => None,
        }
    }

    pub fn raw_tcp_opcode(&self) -> Option<u64> {
        self.raw_tcp_meta().map(|meta| meta.opcode)
    }

    #[cfg(feature = "http-types")]
    pub fn http_meta(&self) -> Option<&HttpRequestMeta> {
        match &self.meta {
            NacelleRequestMeta::RawTcp(_) => None,
            NacelleRequestMeta::Http(meta) => Some(meta),
        }
    }
}

enum NacelleBodySource {
    // Single-chunk bodies (the common case for small payloads): avoids Vec/Box heap alloc.
    SingleChunk(Option<Bytes>),
    Buffered {
        chunks: Box<[Bytes]>,
        next_index: usize,
    },
    Streaming {
        receiver: mpsc::Receiver<Result<Bytes, NacelleError>>,
    },
}

pub struct NacelleBody {
    source: NacelleBodySource,
    remaining_bytes: usize,
    _memory_reservation: Option<MemoryReservation>,
}

impl NacelleBody {
    #[doc(hidden)]
    pub fn new(
        receiver: mpsc::Receiver<Result<Bytes, NacelleError>>,
        remaining_bytes: usize,
    ) -> Self {
        Self {
            source: NacelleBodySource::Streaming { receiver },
            remaining_bytes,
            _memory_reservation: None,
        }
    }

    pub fn empty() -> Self {
        Self {
            source: NacelleBodySource::Buffered {
                chunks: Box::new([]),
                next_index: 0,
            },
            remaining_bytes: 0,
            _memory_reservation: None,
        }
    }

    pub fn bytes(chunk: impl Into<Bytes>) -> Self {
        let chunk = chunk.into();
        let remaining_bytes = chunk.len();
        if remaining_bytes == 0 {
            return Self::empty();
        }
        Self {
            source: NacelleBodySource::SingleChunk(Some(chunk)),
            remaining_bytes,
            _memory_reservation: None,
        }
    }

    pub fn channel(capacity: usize) -> (mpsc::Sender<Result<Bytes, NacelleError>>, NacelleBody) {
        let (tx, rx) = mpsc::channel(capacity.max(1));
        (tx, NacelleBody::new(rx, 0))
    }

    #[doc(hidden)]
    pub fn from_single_chunk(chunk: Bytes, remaining_bytes: usize) -> Self {
        Self {
            source: NacelleBodySource::SingleChunk(Some(chunk)),
            remaining_bytes,
            _memory_reservation: None,
        }
    }

    #[doc(hidden)]
    pub fn from_buffered(chunks: Vec<Bytes>, remaining_bytes: usize) -> Self {
        Self {
            source: NacelleBodySource::Buffered {
                chunks: chunks.into_boxed_slice(),
                next_index: 0,
            },
            remaining_bytes,
            _memory_reservation: None,
        }
    }

    #[doc(hidden)]
    pub fn with_memory_reservation(mut self, reservation: MemoryReservation) -> Self {
        self._memory_reservation = Some(reservation);
        self
    }

    #[doc(hidden)]
    pub fn try_into_single_chunk_or_empty(self) -> Result<Option<Bytes>, Self> {
        match self.source {
            NacelleBodySource::SingleChunk(chunk) => Ok(chunk),
            NacelleBodySource::Buffered { chunks, next_index } => {
                let remaining = chunks.len().saturating_sub(next_index);
                if remaining == 0 {
                    Ok(None)
                } else if remaining == 1 {
                    Ok(Some(chunks[next_index].clone()))
                } else {
                    Err(Self {
                        source: NacelleBodySource::Buffered { chunks, next_index },
                        remaining_bytes: self.remaining_bytes,
                        _memory_reservation: self._memory_reservation,
                    })
                }
            }
            NacelleBodySource::Streaming { receiver } => Err(Self {
                source: NacelleBodySource::Streaming { receiver },
                remaining_bytes: self.remaining_bytes,
                _memory_reservation: self._memory_reservation,
            }),
        }
    }

    pub fn remaining_bytes(&self) -> usize {
        self.remaining_bytes
    }

    pub async fn next_chunk(&mut self) -> Option<Result<Bytes, NacelleError>> {
        match &mut self.source {
            NacelleBodySource::SingleChunk(slot) => {
                let chunk = slot.take()?;
                self.remaining_bytes = 0;
                Some(Ok(chunk))
            }
            NacelleBodySource::Buffered { chunks, next_index } => {
                let chunk = chunks.get(*next_index)?.clone();
                *next_index += 1;
                self.remaining_bytes = self.remaining_bytes.saturating_sub(chunk.len());
                Some(Ok(chunk))
            }
            NacelleBodySource::Streaming { receiver } => match receiver.recv().await {
                Some(Ok(chunk)) => {
                    self.remaining_bytes = self.remaining_bytes.saturating_sub(chunk.len());
                    Some(Ok(chunk))
                }
                other => other,
            },
        }
    }
}

impl Stream for NacelleBody {
    type Item = Result<Bytes, NacelleError>;

    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        match &mut self.source {
            NacelleBodySource::SingleChunk(slot) => {
                let Some(chunk) = slot.take() else {
                    return Poll::Ready(None);
                };
                self.remaining_bytes = 0;
                Poll::Ready(Some(Ok(chunk)))
            }
            NacelleBodySource::Buffered { chunks, next_index } => {
                let Some(chunk) = chunks.get(*next_index).cloned() else {
                    return Poll::Ready(None);
                };
                *next_index += 1;
                self.remaining_bytes = self.remaining_bytes.saturating_sub(chunk.len());
                Poll::Ready(Some(Ok(chunk)))
            }
            NacelleBodySource::Streaming { receiver } => match receiver.poll_recv(cx) {
                Poll::Ready(Some(Ok(chunk))) => {
                    self.remaining_bytes = self.remaining_bytes.saturating_sub(chunk.len());
                    Poll::Ready(Some(Ok(chunk)))
                }
                other => other,
            },
        }
    }
}
