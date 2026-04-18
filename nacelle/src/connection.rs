use std::cell::RefCell;
use std::marker::PhantomData;
use std::sync::Arc;

use bytes::{Bytes, BytesMut};
use tokio::sync::mpsc;

use crate::runtime::{MaybeSend, NacelleRead, NacelleWrite};

use crate::config::NacelleConfig;
use crate::error::NacelleError;
use crate::handler::BoxedHandler;
use crate::protocol::{DecodedRequest, Protocol};
use crate::registry::HandlerRegistry;
use crate::request::{RequestBody, RequestMetadata, ResponseSink, ResponseWriter};

// One pooled encode buffer per OS thread. Tokio may move tasks between worker
// threads across await points, but buffers naturally migrate toward the threads
// that use them most, eliminating the per-request heap allocation after warmup.
thread_local! {
    static ENCODE_BUF_POOL: RefCell<Option<BytesMut>> = const { RefCell::new(None) };
}

#[inline]
fn take_encode_buf(capacity: usize) -> BytesMut {
    ENCODE_BUF_POOL.with(|pool| {
        pool.borrow_mut()
            .take()
            .unwrap_or_else(|| BytesMut::with_capacity(capacity))
    })
}

#[inline]
fn return_encode_buf(mut buf: BytesMut, max_capacity: usize) {
    buf.clear();
    // Only pool the buffer when it fits within the configured response capacity.
    // Oversized buffers (grown to serve a large response) are dropped here so
    // threads don't permanently retain e.g. 10 MB of heap per worker thread.
    if buf.capacity() <= max_capacity {
        ENCODE_BUF_POOL.with(|pool| {
            *pool.borrow_mut() = Some(buf);
        });
    }
}

struct ProtocolResponseSink<Req, P>
where
    Req: RequestMetadata,
    P: Protocol<Req>,
{
    protocol: Arc<P>,
    // Raw pointer to the connection-owned write buffer.  Safety: the buffer
    // lives in serve_connection and outlives every request; accesses within
    // finish() are sequential with the connection loop (fast path) or complete
    // before the connection loop reads write_buf again (slow/spawned path).
    write_buf: *mut BytesMut,
    context: P::ResponseContext,
    encode_buffer: BytesMut,
    // Maximum capacity to retain in the thread-local encode buffer pool.
    // Buffers that grew beyond this (e.g. for a large response) are dropped
    // instead of being pooled, so worker threads don't permanently hold
    // large allocations after serving an unusually large response.
    max_pool_encode_buf: usize,
    pending_chunk: Option<Bytes>,
    _req: PhantomData<fn() -> Req>,
}

impl<Req, P> ResponseSink for ProtocolResponseSink<Req, P>
where
    Req: RequestMetadata,
    P: Protocol<Req> + Send + Sync + 'static,
{
    fn write_bytes(&mut self, chunk: Bytes) -> Result<(), NacelleError> {
        // Encode the previous pending chunk as a non-terminal frame into the accumulation
        // buffer. The incoming chunk is held in reserve so we always know which frame is
        // terminal (needed to set the correct END flag). No I/O or locking here.
        if let Some(prev) = self.pending_chunk.replace(chunk) {
            self.encode_buffer.reserve(prev.len() + 32);
            self.protocol.encode_response_chunk(
                &mut self.context,
                prev,
                &mut self.encode_buffer,
            )?;
        }
        Ok(())
    }

    fn finish(&mut self) -> Result<(), NacelleError> {
        // Encode the last pending chunk as a terminal frame (chunk + END flag in one
        // frame), or write a bare END frame if the handler wrote no data.
        if let Some(last) = self.pending_chunk.take() {
            self.encode_buffer.reserve(last.len() + 32);
            self.protocol.encode_response_terminal_chunk(
                &mut self.context,
                last,
                &mut self.encode_buffer,
            )?;
        } else {
            self.encode_buffer.reserve(32);
            self.protocol
                .encode_response_end(&mut self.context, &mut self.encode_buffer)?;
        }
        // Append encoded bytes directly to the connection's write buffer — zero
        // allocation, no channel, no cross-task wake.  Safety: see write_buf field comment.
        if !self.encode_buffer.is_empty() {
            let wb = unsafe { &mut *self.write_buf };
            wb.extend_from_slice(&self.encode_buffer);
            return_encode_buf(std::mem::take(&mut self.encode_buffer), self.max_pool_encode_buf);
        } else {
            return_encode_buf(std::mem::take(&mut self.encode_buffer), self.max_pool_encode_buf);
        }
        Ok(())
    }
}

// Safety: ProtocolResponseSink is sent to a spawned handler task only on the
// slow (streaming-body) path.  In that path the handler task is the sole writer
// of write_buf while the connection task runs pump_request_body (which never
// touches write_buf).  The tokio join fence provides happens-before between the
// handler's writes and the connection task's subsequent reads of write_buf.
unsafe impl<Req: RequestMetadata, P: Protocol<Req> + Send + Sync + 'static> Send
    for ProtocolResponseSink<Req, P>
{
}

/// Per-connection writer task. Receives encoded response frames from concurrent
/// request handlers and writes them to the TCP stream, coalescing multiple frames
/// into one syscall when they arrive close together.
pub async fn serve_connection<Svc, Req, P, R, W>(
    mut reader: R,
    mut writer: W,
    service: Arc<Svc>,
    protocol: Arc<P>,
    registry: Arc<HandlerRegistry<Svc, Req>>,
    config: NacelleConfig,
) -> Result<(), NacelleError>
where
    Svc: Send + Sync + 'static,
    Req: RequestMetadata + Send + 'static,
    P: Protocol<Req> + Send + Sync + 'static,
    R: NacelleRead + MaybeSend + 'static,
    W: NacelleWrite + MaybeSend + 'static,
{
    let mut read_buf = BytesMut::with_capacity(config.read_buffer_capacity);
    // Per-connection accumulation buffer: responses are appended here synchronously
    // during request processing, then flushed in one write syscall before the next
    // blocking socket read.  Starts empty; capacity grows on first use.
    // Pre-allocate to match the configured response buffer size so that a full
    // 64 KB flush fits without reallocation on the first write cycle.
    let mut write_buf = BytesMut::with_capacity(config.response_buffer_capacity);

    let result: Result<(), NacelleError> = async {
        'conn: loop {
            // Flush all accumulated responses before we block waiting for more data.
            // Doing it here — rather than after every request — lets pipelined responses
            // coalesce into a single write syscall.
            if !write_buf.is_empty() {
                writer.write_all(&write_buf).await?;
                write_buf.clear();
                // If write_buf grew to serve a large response, release the excess
                // capacity so idle connections don't permanently retain their
                // high-water-mark allocation while blocked waiting for the next request.
                if write_buf.capacity() > config.response_buffer_capacity {
                    write_buf = BytesMut::with_capacity(config.response_buffer_capacity);
                }
            }

            // If read_buf is empty and grew to buffer a large request body, release
            // the excess capacity before we block on the next read, for the same
            // reason write_buf is shrunk above.  Only possible when empty; partial
            // frame data must be preserved.
            if read_buf.is_empty() && read_buf.capacity() > config.read_buffer_capacity {
                read_buf = BytesMut::with_capacity(config.read_buffer_capacity);
            }

            // Block until at least one more byte of request data arrives.
            let bytes_read = reader.read_buf(&mut read_buf).await?;
            if bytes_read == 0 {
                if read_buf.is_empty() {
                    break 'conn;
                }
                return Err(NacelleError::UnexpectedEof);
            }

            // Decode and process every complete request already in read_buf without
            // yielding back to the executor.  This is the hot loop for pipelined
            // connections: all pipelined requests are processed back-to-back and their
            // responses accumulate in write_buf, then flushed together above.
            while let Some(decoded) = protocol.decode_head(&mut read_buf, config.max_frame_len)? {
                let opcode = decoded.request.opcode();
                let error_context = protocol.error_context(&decoded.request);
                let Some(handler) = registry.resolve(opcode) else {
                    if can_buffer_request_body(decoded.body_len, &config) {
                        ensure_body_buffered(&mut reader, &mut read_buf, decoded.body_len).await?;
                        drop(buffered_request_body(
                            &mut read_buf,
                            decoded.body_len,
                            config.request_body_chunk_size,
                        ));
                    } else {
                        discard_body(&mut reader, &mut read_buf, decoded.body_len, &config).await?;
                    }
                    write_error::<Req, P>(
                        &mut write_buf,
                        protocol.clone(),
                        Some(error_context),
                        NacelleError::UnknownOpcode(opcode),
                        config.response_buffer_capacity,
                    )?;
                    continue;
                };

                run_request(
                    &mut reader,
                    &mut read_buf,
                    &mut write_buf,
                    service.clone(),
                    protocol.clone(),
                    handler,
                    decoded,
                    error_context,
                    &config,
                )
                .await?;
            }
        }

        Ok(())
    }
    .await;

    // Best-effort final flush so the client sees all pending response bytes.
    // write_buf is dropped immediately after, so no need to shrink it here.
    if !write_buf.is_empty() {
        let _ = writer.write_all(&write_buf).await;
    }

    result
}

struct HandlerOutcome {
    wrote_response: bool,
    error: Option<NacelleError>,
}

#[allow(clippy::too_many_arguments)]
async fn run_request<Svc, Req, P, R>(
    reader: &mut R,
    read_buf: &mut BytesMut,
    write_buf: &mut BytesMut,
    service: Arc<Svc>,
    protocol: Arc<P>,
    handler: &BoxedHandler<Svc, Req>,
    decoded: DecodedRequest<Req>,
    error_context: P::ErrorContext,
    config: &NacelleConfig,
) -> Result<(), NacelleError>
where
    Svc: Send + Sync + 'static,
    Req: RequestMetadata + Send + 'static,
    P: Protocol<Req> + Send + Sync + 'static,
    R: NacelleRead + MaybeSend,
{
    let request = decoded.request;
    let outcome = if decoded.body_len <= read_buf.len() {
        let response = make_response_writer(
            protocol.clone(),
            write_buf as *mut BytesMut,
            &request,
            config.response_buffer_capacity,
        );
        let body =
            buffered_request_body(read_buf, decoded.body_len, config.request_body_chunk_size);
        execute_handler(handler, service, request, body, response).await?
    } else {
        let response = make_response_writer(
            protocol.clone(),
            write_buf as *mut BytesMut,
            &request,
            config.response_buffer_capacity,
        );
        let response_probe = response.clone();
        let (body_tx, body_rx) = mpsc::channel(config.request_body_channel_capacity);
        let body = RequestBody::new(body_rx, decoded.body_len);
        // Clone only for the spawn path (streaming body — uncommon).
        let h = handler.clone();
        let handler_task = crate::runtime::spawn(async move {
            execute_handler(&h, service, request, body, response_probe).await
        });

        let pump_result =
            pump_request_body(reader, read_buf, decoded.body_len, &body_tx, config).await;
        drop(body_tx);
        let outcome = handler_task.await??;
        pump_result?;
        outcome
    };

    if let Some(error) = outcome.error
        && !outcome.wrote_response
    {
        write_error::<Req, P>(
            write_buf,
            protocol,
            Some(error_context),
            error,
            config.response_buffer_capacity,
        )?;
    }

    Ok(())
}

async fn execute_handler<Svc, Req>(
    handler: &BoxedHandler<Svc, Req>,
    service: Arc<Svc>,
    request: Req,
    body: RequestBody,
    response: ResponseWriter,
) -> Result<HandlerOutcome, NacelleError>
where
    Svc: Send + Sync + 'static,
    Req: RequestMetadata + Send + 'static,
{
    let result = handler.call(service, request, body, response.clone()).await;
    let wrote_response = response.has_written();
    match result {
        Ok(()) => {
            response.finish()?;
            Ok(HandlerOutcome {
                wrote_response,
                error: None,
            })
        }
        Err(error) => {
            if wrote_response {
                response.finish()?;
            }
            Ok(HandlerOutcome {
                wrote_response,
                error: Some(error),
            })
        }
    }
}

async fn ensure_body_buffered<R>(
    reader: &mut R,
    read_buf: &mut BytesMut,
    body_len: usize,
) -> Result<(), NacelleError>
where
    R: NacelleRead,
{
    while read_buf.len() < body_len {
        let bytes_read = reader.read_buf(read_buf).await?;
        if bytes_read == 0 {
            return Err(NacelleError::UnexpectedEof);
        }
    }
    Ok(())
}

async fn pump_request_body<R>(
    reader: &mut R,
    read_buf: &mut BytesMut,
    body_len: usize,
    tx: &mpsc::Sender<Result<Bytes, NacelleError>>,
    config: &NacelleConfig,
) -> Result<(), NacelleError>
where
    R: NacelleRead,
{
    let mut remaining = body_len;
    let mut receiver_open = true;

    while remaining > 0 && !read_buf.is_empty() {
        let take = remaining
            .min(read_buf.len())
            .min(config.request_body_chunk_size);
        let chunk = read_buf.split_to(take).freeze();
        remaining -= take;
        if receiver_open && tx.send(Ok(chunk)).await.is_err() {
            receiver_open = false;
        }
    }

    while remaining > 0 {
        let next_len = remaining.min(config.request_body_chunk_size);
        let mut chunk = BytesMut::with_capacity(next_len);
        let bytes_read = reader.read_buf(&mut chunk).await?;
        if bytes_read == 0 {
            if receiver_open {
                let _ = tx.send(Err(NacelleError::UnexpectedEof)).await;
            }
            return Err(NacelleError::UnexpectedEof);
        }

        remaining -= bytes_read;
        if receiver_open && tx.send(Ok(chunk.freeze())).await.is_err() {
            receiver_open = false;
        }
    }

    Ok(())
}

async fn discard_body<R>(
    reader: &mut R,
    read_buf: &mut BytesMut,
    body_len: usize,
    config: &NacelleConfig,
) -> Result<(), NacelleError>
where
    R: NacelleRead,
{
    let (tx, _rx) = mpsc::channel(1);
    pump_request_body(reader, read_buf, body_len, &tx, config).await
}

fn buffered_request_body(
    read_buf: &mut BytesMut,
    body_len: usize,
    chunk_size: usize,
) -> RequestBody {
    if body_len == 0 {
        return RequestBody::from_buffered(Vec::new(), 0);
    }

    // Common case: body fits in one chunk — use SingleChunk to avoid Vec/Box alloc.
    if body_len <= chunk_size {
        let chunk = read_buf.split_to(body_len).freeze();
        return RequestBody::from_single_chunk(chunk, body_len);
    }

    let mut remaining = body_len;
    let mut chunks = Vec::with_capacity(body_len.div_ceil(chunk_size));
    while remaining > 0 {
        let take = remaining.min(chunk_size);
        chunks.push(read_buf.split_to(take).freeze());
        remaining -= take;
    }

    RequestBody::from_buffered(chunks, body_len)
}

fn can_buffer_request_body(body_len: usize, config: &NacelleConfig) -> bool {
    body_len <= config.max_buffered_request_body_per_request
}

fn make_response_writer<Req, P>(
    protocol: Arc<P>,
    write_buf: *mut BytesMut,
    req: &Req,
    buffer_capacity: usize,
) -> ResponseWriter
where
    Req: RequestMetadata,
    P: Protocol<Req> + Send + Sync + 'static,
{
    // Call response_context first (borrows protocol), then move protocol into the
    // struct — avoids a redundant Arc clone.
    let context = protocol.response_context(req);
    ResponseWriter::new(Box::new(ProtocolResponseSink::<Req, P> {
        protocol,
        write_buf,
        context,
        encode_buffer: take_encode_buf(buffer_capacity.max(32)),
        max_pool_encode_buf: buffer_capacity,
        pending_chunk: None,
        _req: PhantomData,
    }))
}

fn write_error<Req, P>(
    write_buf: &mut BytesMut,
    protocol: Arc<P>,
    context: Option<P::ErrorContext>,
    error: NacelleError,
    buffer_capacity: usize,
) -> Result<(), NacelleError>
where
    Req: RequestMetadata,
    P: Protocol<Req> + Send + Sync + 'static,
{
    let prev_len = write_buf.len();
    write_buf.reserve(buffer_capacity.max(128));
    if let Err(e) = protocol.encode_error(context.as_ref(), &error, write_buf) {
        write_buf.truncate(prev_len);
        return Err(e);
    }
    Ok(())
}
