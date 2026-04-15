use std::cell::UnsafeCell;
use std::pin::Pin;
use std::sync::Arc;
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
    // Single-chunk bodies (the common case for small payloads): avoids Vec/Box heap alloc.
    SingleChunk(Option<Bytes>),
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

    pub(crate) fn from_single_chunk(chunk: Bytes, remaining_bytes: usize) -> Self {
        Self {
            source: RequestBodySource::SingleChunk(Some(chunk)),
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
            RequestBodySource::SingleChunk(slot) => {
                let chunk = slot.take()?;
                self.remaining_bytes = 0;
                Some(Ok(chunk))
            }
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
            RequestBodySource::SingleChunk(slot) => {
                let Some(chunk) = slot.take() else {
                    return Poll::Ready(None);
                };
                self.remaining_bytes = 0;
                Poll::Ready(Some(Ok(chunk)))
            }
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

pub(crate) trait ResponseSink: Send {
    /// Encode `chunk` into the response buffer. Must not perform I/O — callers rely on this
    /// being synchronous and allocation-free on the hot path.
    fn write_bytes(&mut self, chunk: Bytes) -> Result<(), CascadeError>;
    /// Flush all buffered chunks to the underlying writer. Called exactly once per response.
    /// Synchronous: implementations must use non-blocking I/O (e.g. unbounded channel send).
    fn finish(&mut self) -> Result<(), CascadeError>;
}

struct ResponseWriterInner {
    // Access discipline (upheld by the framework, not the type system):
    //   • write_bytes — only called from within the handler future; calls are sequential
    //     within that single task, never concurrent with finish or each other.
    //   • finish / has_written — only called after the handler future has completed
    //     (either directly awaited in the same task, or after join on the spawned task).
    //
    // Because these two phases never overlap, UnsafeCell access is sound even though
    // ResponseWriter is Clone + Send + Sync.  The AtomicBools provide the memory-ordering
    // fence needed when the handler runs in a spawned task (tokio task-join implies
    // happens-before, so Relaxed ordering is sufficient for `wrote`; AcqRel on `finished`
    // guards the sink take in finish()).
    sink: UnsafeCell<Option<Box<dyn ResponseSink>>>,
    finished: AtomicBool,
    wrote: AtomicBool,
}

// Safety: see access discipline comment on ResponseWriterInner.
unsafe impl Send for ResponseWriterInner {}
unsafe impl Sync for ResponseWriterInner {}

#[derive(Clone)]
pub struct ResponseWriter {
    inner: Arc<ResponseWriterInner>,
}

impl ResponseWriter {
    pub(crate) fn new(sink: Box<dyn ResponseSink>) -> Self {
        Self {
            inner: Arc::new(ResponseWriterInner {
                sink: UnsafeCell::new(Some(sink)),
                finished: AtomicBool::new(false),
                wrote: AtomicBool::new(false),
            }),
        }
    }

    pub fn write_bytes(&self, chunk: impl Into<Bytes>) -> Result<(), CascadeError> {
        let chunk = chunk.into();
        if chunk.is_empty() {
            return Ok(());
        }

        if self.inner.finished.load(Ordering::Relaxed) {
            return Err(CascadeError::ConnectionClosed);
        }

        // Safety: write_bytes is only called from within the handler future, which is
        // sequential (no concurrent write_bytes calls).  finish() only accesses sink
        // after the handler future has completed.  These phases never overlap.
        let sink = unsafe { &mut *self.inner.sink.get() };
        sink.as_mut()
            .ok_or(CascadeError::ConnectionClosed)?
            .write_bytes(chunk)?;

        self.inner.wrote.store(true, Ordering::Relaxed);
        Ok(())
    }

    pub fn finish(&self) -> Result<(), CascadeError> {
        // AcqRel: Acquire sees all preceding write_bytes stores; Release makes the
        // finished state visible to any other caller of finish (idempotency guard).
        if self.inner.finished.swap(true, Ordering::AcqRel) {
            return Ok(());
        }

        // Safety: finished swap above guarantees this block runs at most once, and it
        // only runs after the handler future has completed (no concurrent write_bytes).
        let sink = unsafe { &mut *self.inner.sink.get() }.take();
        if let Some(mut s) = sink {
            s.finish()?;
        }
        Ok(())
    }

    pub fn has_written(&self) -> bool {
        self.inner.wrote.load(Ordering::Relaxed)
    }
}
