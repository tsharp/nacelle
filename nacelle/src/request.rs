use std::pin::Pin;
use std::task::{Context, Poll};

use bytes::Bytes;
use futures_core::Stream;
use tokio::sync::mpsc;

use crate::error::NacelleError;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RawTcpRequestMeta {
    pub request_id: Option<u64>,
    pub opcode: u64,
    pub flags: u32,
    pub body_len: usize,
}

#[cfg(feature = "http")]
#[derive(Debug, Clone)]
pub struct HttpRequestMeta {
    pub method: http::Method,
    pub uri: http::Uri,
    pub headers: http::HeaderMap,
}

#[derive(Debug, Clone)]
pub enum NacelleRequestMeta {
    RawTcp(RawTcpRequestMeta),
    #[cfg(feature = "http")]
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
    pub meta: NacelleRequestMeta,
    pub body: NacelleBody,
}

impl NacelleRequest {
    pub fn raw_tcp_meta(&self) -> Option<&RawTcpRequestMeta> {
        match &self.meta {
            NacelleRequestMeta::RawTcp(meta) => Some(meta),
            #[cfg(feature = "http")]
            NacelleRequestMeta::Http(_) => None,
        }
    }

    pub fn raw_tcp_opcode(&self) -> Option<u64> {
        self.raw_tcp_meta().map(|meta| meta.opcode)
    }

    #[cfg(feature = "http")]
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
}

impl NacelleBody {
    pub(crate) fn new(
        receiver: mpsc::Receiver<Result<Bytes, NacelleError>>,
        remaining_bytes: usize,
    ) -> Self {
        Self {
            source: NacelleBodySource::Streaming { receiver },
            remaining_bytes,
        }
    }

    pub fn empty() -> Self {
        Self {
            source: NacelleBodySource::Buffered {
                chunks: Box::new([]),
                next_index: 0,
            },
            remaining_bytes: 0,
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
        }
    }

    pub fn channel(capacity: usize) -> (mpsc::Sender<Result<Bytes, NacelleError>>, NacelleBody) {
        let (tx, rx) = mpsc::channel(capacity.max(1));
        (tx, NacelleBody::new(rx, 0))
    }

    #[cfg(feature = "raw_tcp")]
    pub(crate) fn from_single_chunk(chunk: Bytes, remaining_bytes: usize) -> Self {
        Self {
            source: NacelleBodySource::SingleChunk(Some(chunk)),
            remaining_bytes,
        }
    }

    #[cfg(feature = "raw_tcp")]
    pub(crate) fn from_buffered(chunks: Vec<Bytes>, remaining_bytes: usize) -> Self {
        Self {
            source: NacelleBodySource::Buffered {
                chunks: chunks.into_boxed_slice(),
                next_index: 0,
            },
            remaining_bytes,
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
