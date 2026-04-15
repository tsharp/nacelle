use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::task::{Context, Poll};

use bytes::Bytes;
use futures_core::Stream;
use tokio::sync::{Mutex, mpsc};

use crate::error::CascadeError;

pub trait RequestMetadata: Send + 'static {
    fn opcode(&self) -> u64;
}

pub struct RequestBody {
    receiver: mpsc::Receiver<Result<Bytes, CascadeError>>,
    remaining_bytes: usize,
}

impl RequestBody {
    pub(crate) fn new(receiver: mpsc::Receiver<Result<Bytes, CascadeError>>, remaining_bytes: usize) -> Self {
        Self {
            receiver,
            remaining_bytes,
        }
    }

    pub fn remaining_bytes(&self) -> usize {
        self.remaining_bytes
    }

    pub async fn next_chunk(&mut self) -> Option<Result<Bytes, CascadeError>> {
        match self.receiver.recv().await {
            Some(Ok(chunk)) => {
                self.remaining_bytes = self.remaining_bytes.saturating_sub(chunk.len());
                Some(Ok(chunk))
            }
            other => other,
        }
    }
}

impl Stream for RequestBody {
    type Item = Result<Bytes, CascadeError>;

    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        match self.receiver.poll_recv(cx) {
            Poll::Ready(Some(Ok(chunk))) => {
                self.remaining_bytes = self.remaining_bytes.saturating_sub(chunk.len());
                Poll::Ready(Some(Ok(chunk)))
            }
            other => other,
        }
    }
}

pub(crate) type SinkFuture<'a> = Pin<Box<dyn Future<Output = Result<(), CascadeError>> + Send + 'a>>;

pub(crate) trait ResponseSink: Send {
    fn write_bytes<'a>(&'a mut self, chunk: Bytes) -> SinkFuture<'a>;
    fn finish<'a>(&'a mut self) -> SinkFuture<'a>;
}

struct ResponseState {
    sink: Box<dyn ResponseSink>,
    finished: bool,
}

#[derive(Clone)]
pub struct ResponseWriter {
    state: Arc<Mutex<ResponseState>>,
    wrote: Arc<AtomicBool>,
}

impl ResponseWriter {
    pub(crate) fn new(sink: Box<dyn ResponseSink>) -> Self {
        Self {
            state: Arc::new(Mutex::new(ResponseState {
                sink,
                finished: false,
            })),
            wrote: Arc::new(AtomicBool::new(false)),
        }
    }

    pub async fn write_bytes(&self, chunk: impl Into<Bytes>) -> Result<(), CascadeError> {
        let chunk = chunk.into();
        if chunk.is_empty() {
            return Ok(());
        }

        let mut state = self.state.lock().await;
        if state.finished {
            return Err(CascadeError::ConnectionClosed);
        }

        state.sink.write_bytes(chunk).await?;
        self.wrote.store(true, Ordering::Release);
        Ok(())
    }

    pub async fn finish(&self) -> Result<(), CascadeError> {
        let mut state = self.state.lock().await;
        if state.finished {
            return Ok(());
        }

        state.sink.finish().await?;
        state.finished = true;
        Ok(())
    }

    pub fn has_written(&self) -> bool {
        self.wrote.load(Ordering::Acquire)
    }
}
