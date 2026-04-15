use std::marker::PhantomData;
use std::sync::Arc;

use bytes::{Bytes, BytesMut};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
use tokio::sync::{Mutex, mpsc};

use crate::config::CascadeConfig;
use crate::error::CascadeError;
use crate::handler::BoxedHandler;
use crate::protocol::{DecodedRequest, Protocol};
use crate::registry::HandlerRegistry;
use crate::request::{RequestBody, RequestMetadata, ResponseSink, ResponseWriter};

struct ProtocolResponseSink<Req, P, W>
where
    Req: RequestMetadata,
    P: Protocol<Req>,
{
    protocol: Arc<P>,
    writer: Arc<Mutex<W>>,
    context: P::ResponseContext,
    buffer_capacity: usize,
    _req: PhantomData<fn() -> Req>,
}

impl<Req, P, W> ResponseSink for ProtocolResponseSink<Req, P, W>
where
    Req: RequestMetadata,
    P: Protocol<Req> + Send + Sync + 'static,
    W: AsyncWrite + Unpin + Send + 'static,
{
    fn write_bytes<'a>(&'a mut self, chunk: Bytes) -> crate::request::SinkFuture<'a> {
        Box::pin(async move {
            let mut dst = BytesMut::with_capacity(self.buffer_capacity.max(chunk.len() + 32));
            self.protocol
                .encode_response_chunk(&mut self.context, chunk, &mut dst)?;
            if !dst.is_empty() {
                let mut writer = self.writer.lock().await;
                writer.write_all(&dst).await?;
            }
            Ok(())
        })
    }

    fn finish<'a>(&'a mut self) -> crate::request::SinkFuture<'a> {
        Box::pin(async move {
            let mut dst = BytesMut::with_capacity(self.buffer_capacity.max(32));
            self.protocol.encode_response_end(&mut self.context, &mut dst)?;
            let mut writer = self.writer.lock().await;
            if !dst.is_empty() {
                writer.write_all(&dst).await?;
            }
            writer.flush().await?;
            Ok(())
        })
    }
}

pub async fn serve_connection<Svc, Req, P, IO>(
    io: IO,
    service: Arc<Svc>,
    protocol: Arc<P>,
    registry: Arc<HandlerRegistry<Svc, Req>>,
    config: CascadeConfig,
) -> Result<(), CascadeError>
where
    Svc: Send + Sync + 'static,
    Req: RequestMetadata + Send + 'static,
    P: Protocol<Req> + Send + Sync + 'static,
    IO: AsyncRead + AsyncWrite + Unpin + Send + 'static,
{
    let (mut reader, writer) = tokio::io::split(io);
    let shared_writer = Arc::new(Mutex::new(writer));
    let mut read_buf = BytesMut::with_capacity(config.read_buffer_capacity);

    loop {
        let Some(decoded) =
            read_decoded_request(&mut reader, &mut read_buf, protocol.as_ref(), &config).await?
        else {
            return Ok(());
        };

        let opcode = decoded.request.opcode();
        let error_context = protocol.error_context(&decoded.request);
        let Some(handler) = registry.resolve(opcode).cloned() else {
            discard_body(&mut reader, &mut read_buf, decoded.body_len, &config).await?;
            write_error::<Req, P, _>(
                shared_writer.clone(),
                protocol.clone(),
                Some(error_context),
                CascadeError::UnknownOpcode(opcode),
                config.response_buffer_capacity,
            )
            .await?;
            continue;
        };

        run_request(
            &mut reader,
            &mut read_buf,
            shared_writer.clone(),
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

struct HandlerOutcome {
    wrote_response: bool,
    error: Option<CascadeError>,
}

async fn run_request<Svc, Req, P, R, W>(
    reader: &mut R,
    read_buf: &mut BytesMut,
    writer: Arc<Mutex<W>>,
    service: Arc<Svc>,
    protocol: Arc<P>,
    handler: BoxedHandler<Svc, Req>,
    decoded: DecodedRequest<Req>,
    error_context: P::ErrorContext,
    config: &CascadeConfig,
) -> Result<(), CascadeError>
where
    Svc: Send + Sync + 'static,
    Req: RequestMetadata + Send + 'static,
    P: Protocol<Req> + Send + Sync + 'static,
    R: AsyncRead + Unpin + Send,
    W: AsyncWrite + Unpin + Send + 'static,
{
    let response = make_response_writer(
        protocol.clone(),
        writer.clone(),
        &decoded.request,
        config.response_buffer_capacity,
    );
    let response_probe = response.clone();
    let request = decoded.request;
    let (body_tx, body_rx) = mpsc::channel(config.request_body_channel_capacity);
    let body = RequestBody::new(body_rx, decoded.body_len);
    let handler_task = tokio::spawn(async move {
        let result = handler.call(service, request, body, response).await;
        let wrote_response = response_probe.has_written();
        match result {
            Ok(()) => {
                response_probe.finish().await?;
                Ok::<HandlerOutcome, CascadeError>(HandlerOutcome {
                    wrote_response,
                    error: None,
                })
            }
            Err(error) => {
                if wrote_response {
                    response_probe.finish().await?;
                }
                Ok::<HandlerOutcome, CascadeError>(HandlerOutcome {
                    wrote_response,
                    error: Some(error),
                })
            }
        }
    });

    let pump_result = pump_request_body(reader, read_buf, decoded.body_len, &body_tx, config).await;
    drop(body_tx);
    let outcome = handler_task.await??;
    pump_result?;

    if let Some(error) = outcome.error {
        if !outcome.wrote_response {
            write_error::<Req, P, W>(
                writer,
                protocol,
                Some(error_context),
                error,
                config.response_buffer_capacity,
            )
            .await?;
        }
    }

    Ok(())
}

async fn read_decoded_request<R, Req, P>(
    reader: &mut R,
    read_buf: &mut BytesMut,
    protocol: &P,
    config: &CascadeConfig,
) -> Result<Option<DecodedRequest<Req>>, CascadeError>
where
    R: AsyncRead + Unpin,
    Req: RequestMetadata,
    P: Protocol<Req>,
{
    loop {
        if let Some(decoded) = protocol.decode_head(read_buf, config.max_frame_len)? {
            return Ok(Some(decoded));
        }

        let bytes_read = reader.read_buf(read_buf).await?;
        if bytes_read == 0 {
            if read_buf.is_empty() {
                return Ok(None);
            }
            return Err(CascadeError::UnexpectedEof);
        }
    }
}

async fn pump_request_body<R>(
    reader: &mut R,
    read_buf: &mut BytesMut,
    body_len: usize,
    tx: &mpsc::Sender<Result<Bytes, CascadeError>>,
    config: &CascadeConfig,
) -> Result<(), CascadeError>
where
    R: AsyncRead + Unpin,
{
    let mut remaining = body_len;
    let mut receiver_open = true;

    while remaining > 0 && !read_buf.is_empty() {
        let take = remaining.min(read_buf.len()).min(config.request_body_chunk_size);
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
                let _ = tx.send(Err(CascadeError::UnexpectedEof)).await;
            }
            return Err(CascadeError::UnexpectedEof);
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
    config: &CascadeConfig,
) -> Result<(), CascadeError>
where
    R: AsyncRead + Unpin,
{
    let (tx, _rx) = mpsc::channel(1);
    pump_request_body(reader, read_buf, body_len, &tx, config).await
}

fn make_response_writer<Req, P, W>(
    protocol: Arc<P>,
    writer: Arc<Mutex<W>>,
    req: &Req,
    buffer_capacity: usize,
) -> ResponseWriter
where
    Req: RequestMetadata,
    P: Protocol<Req> + Send + Sync + 'static,
    W: AsyncWrite + Unpin + Send + 'static,
{
    ResponseWriter::new(Box::new(ProtocolResponseSink::<Req, P, W> {
        protocol: protocol.clone(),
        writer,
        context: protocol.response_context(req),
        buffer_capacity,
        _req: PhantomData,
    }))
}

async fn write_error<Req, P, W>(
    writer: Arc<Mutex<W>>,
    protocol: Arc<P>,
    context: Option<P::ErrorContext>,
    error: CascadeError,
    buffer_capacity: usize,
) -> Result<(), CascadeError>
where
    Req: RequestMetadata,
    W: AsyncWrite + Unpin + Send + 'static,
    P: Protocol<Req> + Send + Sync + 'static,
{
    let mut dst = BytesMut::with_capacity(buffer_capacity.max(128));
    protocol.encode_error(context.as_ref(), &error, &mut dst)?;
    let mut guard = writer.lock().await;
    guard.write_all(&dst).await?;
    guard.flush().await?;
    Ok(())
}
