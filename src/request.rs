use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};
use std::task::{Context, Poll};

use bytes::Bytes;
use futures_core::Stream;
use tokio::sync::mpsc;

use crate::error::CascadeError;

pub trait RequestMetadata: Send + 'static {
    fn opcode(&self) -> u64;
}

enum RequestBodySource {
    Buffered {
        chunks: Box<[Bytes]>,
        next_index: usize,
    },
    Streaming {
        receiver: mpsc::Receiver<Result<Bytes, CascadeError>>,
    },
}

pub struct RequestBody {
    source: RequestBodySource,
    remaining_bytes: usize,
}

impl RequestBody {
    pub(crate) fn new(receiver: mpsc::Receiver<Result<Bytes, CascadeError>>, remaining_bytes: usize) -> Self {
        Self {
            source: RequestBodySource::Streaming { receiver },
            remaining_bytes,
        }
    }

    pub(crate) fn from_buffered(chunks: Vec<Bytes>, remaining_bytes: usize) -> Self {
        Self {
            source: RequestBodySource::Buffered {
                chunks: chunks.into_boxed_slice(),
                next_index: 0,
            },
            remaining_bytes,
        }
    }

    pub fn remaining_bytes(&self) -> usize {
        self.remaining_bytes
    }

    pub async fn next_chunk(&mut self) -> Option<Result<Bytes, CascadeError>> {
        match &mut self.source {
            RequestBodySource::Buffered { chunks, next_index } => {
                let chunk = chunks.get(*next_index)?.clone();
                *next_index += 1;
                self.remaining_bytes = self.remaining_bytes.saturating_sub(chunk.len());
                Some(Ok(chunk))
            }
            RequestBodySource::Streaming { receiver } => match receiver.recv().await {
                Some(Ok(chunk)) => {
                    self.remaining_bytes = self.remaining_bytes.saturating_sub(chunk.len());
                    Some(Ok(chunk))
                }
                other => other,
            },
        }
    }
}

impl Stream for RequestBody {
    type Item = Result<Bytes, CascadeError>;

    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        match &mut self.source {
            RequestBodySource::Buffered { chunks, next_index } => {
                let Some(chunk) = chunks.get(*next_index).cloned() else {
                    return Poll::Ready(None);
                };
                *next_index += 1;
                self.remaining_bytes = self.remaining_bytes.saturating_sub(chunk.len());
                Poll::Ready(Some(Ok(chunk)))
            }
            RequestBodySource::Streaming { receiver } => match receiver.poll_recv(cx) {
                Poll::Ready(Some(Ok(chunk))) => {
                    self.remaining_bytes = self.remaining_bytes.saturating_sub(chunk.len());
                    Poll::Ready(Some(Ok(chunk)))
                }
                other => other,
            },
        }
    }
}

pub(crate) type SinkFuture<'a> = Pin<Box<dyn Future<Output = Result<(), CascadeError>> + Send + 'a>>;

pub(crate) trait ResponseSink: Send {
    /// Encode `chunk` into the response buffer. Must not perform I/O — callers rely on this
    /// being synchronous and allocation-free on the hot path.
    fn write_bytes(&mut self, chunk: Bytes) -> Result<(), CascadeError>;
    /// Flush all buffered chunks to the underlying writer. Called exactly once per response.
    fn finish<'a>(&'a mut self) -> SinkFuture<'a>;
}

struct ResponseWriterInner {
    // Holds the sink until finish() takes it out. Using std::sync::Mutex because the
    // critical section is purely synchronous — no .await inside the lock.
    sink: Mutex<Option<Box<dyn ResponseSink>>>,
    finished: AtomicBool,
    wrote: AtomicBool,
}

#[derive(Clone)]
pub struct ResponseWriter {
    inner: Arc<ResponseWriterInner>,
}

impl ResponseWriter {
    pub(crate) fn new(sink: Box<dyn ResponseSink>) -> Self {
        Self {
            inner: Arc::new(ResponseWriterInner {
                sink: Mutex::new(Some(sink)),
                finished: AtomicBool::new(false),
                wrote: AtomicBool::new(false),
            }),
        }
    }

    pub async fn write_bytes(&self, chunk: impl Into<Bytes>) -> Result<(), CascadeError> {
        let chunk = chunk.into();
        if chunk.is_empty() {
            return Ok(());
        }

        if self.inner.finished.load(Ordering::Acquire) {
            return Err(CascadeError::ConnectionClosed);
        }

        // Lock is held only for the synchronous write_bytes call — no await inside.
        let mut guard = self.inner.sink.lock().unwrap();
        if self.inner.finished.load(Ordering::Acquire) {
            return Err(CascadeError::ConnectionClosed);
        }
        guard.as_mut().ok_or(CascadeError::ConnectionClosed)?.write_bytes(chunk)?;
        drop(guard);

        self.inner.wrote.store(true, Ordering::Release);
        Ok(())
    }

    pub async fn finish(&self) -> Result<(), CascadeError> {
        if self.inner.finished.swap(true, Ordering::AcqRel) {
            return Ok(());
        }

        // Take the sink out while holding the lock only briefly (no await under lock).
        let mut sink = {
            let mut guard = self.inner.sink.lock().unwrap();
            guard.take()
        };

        if let Some(ref mut s) = sink {
            s.finish().await?;
        }
        Ok(())
    }

    pub fn has_written(&self) -> bool {
        self.inner.wrote.load(Ordering::Acquire)
    }
}
