use std::sync::Arc;

use bytes::{Bytes, BytesMut};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
use tokio::sync::mpsc;

use crate::config::{NacelleConfig, RequestBodyMode};
use crate::error::NacelleError;
use crate::handler::Handler;
use crate::protocol::{DecodedRequest, Protocol};
use crate::request::{NacelleBody, NacelleRequest, NacelleRequestMeta, RequestMetadata};
use crate::response::{NacelleResponse, NacelleResponseMeta};
use crate::telemetry::{NacelleTelemetry, NacelleTransport};

/// Drive one raw TCP framed connection and coalesce completed responses into writes.
pub async fn serve_connection<Req, P, H, R, W>(
    mut reader: R,
    mut writer: W,
    protocol: Arc<P>,
    handler: H,
    config: NacelleConfig,
    telemetry: NacelleTelemetry,
) -> Result<(), NacelleError>
where
    Req: RequestMetadata + Send + 'static,
    P: Protocol<Req> + Send + Sync + 'static,
    H: Handler,
    R: AsyncRead + Unpin + Send + 'static,
    W: AsyncWrite + Unpin + Send + 'static,
{
    let mut read_buf = BytesMut::with_capacity(config.read_buffer_capacity);
    let mut write_buf = BytesMut::with_capacity(config.response_buffer_capacity);
    telemetry.connection_opened(NacelleTransport::RawTcp);

    let result: Result<(), NacelleError> = async {
        'conn: loop {
            if !write_buf.is_empty() {
                writer.write_all(&write_buf).await?;
                write_buf.clear();
                if write_buf.capacity() > config.response_buffer_capacity {
                    write_buf = BytesMut::with_capacity(config.response_buffer_capacity);
                }
            }

            if read_buf.is_empty() && read_buf.capacity() > config.read_buffer_capacity {
                read_buf = BytesMut::with_capacity(config.read_buffer_capacity);
            }

            let bytes_read = reader.read_buf(&mut read_buf).await?;
            if bytes_read == 0 {
                if read_buf.is_empty() {
                    break 'conn;
                }
                return Err(NacelleError::UnexpectedEof);
            }

            while let Some(decoded) = protocol.decode_head(&mut read_buf, config.max_frame_len)? {
                let error_context = protocol.error_context(&decoded.request);
                run_request(
                    &mut reader,
                    &mut read_buf,
                    &mut write_buf,
                    protocol.as_ref(),
                    &handler,
                    decoded,
                    error_context,
                    &config,
                    &telemetry,
                )
                .await?;
            }
        }

        Ok(())
    }
    .await;

    if !write_buf.is_empty() {
        let _ = writer.write_all(&write_buf).await;
    }

    result
}

/// Drive one raw TCP framed connection using a single unsplit I/O object.
pub async fn serve_stream<Req, P, H, IO>(
    mut io: IO,
    protocol: Arc<P>,
    handler: H,
    config: NacelleConfig,
    telemetry: NacelleTelemetry,
) -> Result<(), NacelleError>
where
    Req: RequestMetadata + Send + 'static,
    P: Protocol<Req> + Send + Sync + 'static,
    H: Handler,
    IO: AsyncRead + AsyncWrite + Unpin + Send + 'static,
{
    let mut read_buf = BytesMut::with_capacity(config.read_buffer_capacity);
    let mut write_buf = BytesMut::with_capacity(config.response_buffer_capacity);
    telemetry.connection_opened(NacelleTransport::RawTcp);

    let result: Result<(), NacelleError> = async {
        'conn: loop {
            if !write_buf.is_empty() {
                io.write_all(&write_buf).await?;
                write_buf.clear();
                if write_buf.capacity() > config.response_buffer_capacity {
                    write_buf = BytesMut::with_capacity(config.response_buffer_capacity);
                }
            }

            if read_buf.is_empty() && read_buf.capacity() > config.read_buffer_capacity {
                read_buf = BytesMut::with_capacity(config.read_buffer_capacity);
            }

            let bytes_read = io.read_buf(&mut read_buf).await?;
            if bytes_read == 0 {
                if read_buf.is_empty() {
                    break 'conn;
                }
                return Err(NacelleError::UnexpectedEof);
            }

            while let Some(decoded) = protocol.decode_head(&mut read_buf, config.max_frame_len)? {
                let error_context = protocol.error_context(&decoded.request);
                run_request(
                    &mut io,
                    &mut read_buf,
                    &mut write_buf,
                    protocol.as_ref(),
                    &handler,
                    decoded,
                    error_context,
                    &config,
                    &telemetry,
                )
                .await?;
            }
        }

        Ok(())
    }
    .await;

    if !write_buf.is_empty() {
        let _ = io.write_all(&write_buf).await;
    }

    result
}

#[allow(clippy::too_many_arguments)]
async fn run_request<Req, P, H, R>(
    reader: &mut R,
    read_buf: &mut BytesMut,
    write_buf: &mut BytesMut,
    protocol: &P,
    handler: &H,
    decoded: DecodedRequest<Req>,
    error_context: P::ErrorContext,
    config: &NacelleConfig,
    telemetry: &NacelleTelemetry,
) -> Result<(), NacelleError>
where
    Req: RequestMetadata + Send + 'static,
    P: Protocol<Req> + Send + Sync + 'static,
    H: Handler,
    R: AsyncRead + Unpin + Send,
{
    let request = decoded.request;
    let request_started = std::time::Instant::now();
    let opcode = request.opcode();
    let request_bytes = 4 + 20 + decoded.body_len;
    let response_context = protocol.response_context(&request);
    let outcome = if decoded.body_len <= read_buf.len() {
        let body =
            buffered_request_body(read_buf, decoded.body_len, config.request_body_chunk_size);
        execute_handler(handler, request, decoded.body_len, body).await
    } else if config.request_body_mode == RequestBodyMode::Buffered {
        let body = read_buffered_request_body(reader, read_buf, decoded.body_len).await?;
        execute_handler(handler, request, decoded.body_len, body).await
    } else {
        let (body_tx, body_rx) = mpsc::channel(config.request_body_channel_capacity);
        let body = NacelleBody::new(body_rx, decoded.body_len);
        let h = handler.clone();
        let handler_task = crate::runtime::spawn(async move {
            execute_handler(&h, request, decoded.body_len, body).await
        });

        let pump_result =
            pump_request_body(reader, read_buf, decoded.body_len, &body_tx, config).await;
        drop(body_tx);
        let outcome = handler_task.await??;
        pump_result?;
        Ok(outcome)
    };

    match outcome {
        Ok(response) => {
            let prev_response_len = write_buf.len();
            encode_response_body::<Req, P>(protocol, response_context, response, write_buf).await?;
            telemetry.request_completed(
                NacelleTransport::RawTcp,
                Some(opcode),
                request_bytes,
                write_buf.len().saturating_sub(prev_response_len),
                request_started.elapsed(),
            );
        }
        Err(error) => {
            let prev_response_len = write_buf.len();
            telemetry.request_failed(
                NacelleTransport::RawTcp,
                Some(opcode),
                request_started.elapsed(),
                &error,
            );
            write_error::<Req, P>(
                write_buf,
                protocol,
                Some(error_context),
                error,
                config.response_buffer_capacity,
            )?;
            telemetry.request_completed(
                NacelleTransport::RawTcp,
                Some(opcode),
                request_bytes,
                write_buf.len().saturating_sub(prev_response_len),
                request_started.elapsed(),
            );
        }
    }

    Ok(())
}

async fn read_buffered_request_body<R>(
    reader: &mut R,
    read_buf: &mut BytesMut,
    body_len: usize,
) -> Result<NacelleBody, NacelleError>
where
    R: AsyncRead + Unpin,
{
    if body_len == 0 {
        return Ok(NacelleBody::empty());
    }

    let mut body = BytesMut::with_capacity(body_len);
    if !read_buf.is_empty() {
        let take = body_len.min(read_buf.len());
        body.extend_from_slice(&read_buf.split_to(take));
    }

    while body.len() < body_len {
        let bytes_read = reader.read_buf(&mut body).await?;
        if bytes_read == 0 {
            return Err(NacelleError::UnexpectedEof);
        }
    }

    Ok(NacelleBody::from_single_chunk(body.freeze(), body_len))
}

async fn execute_handler<Req, H>(
    handler: &H,
    request: Req,
    body_len: usize,
    body: NacelleBody,
) -> Result<NacelleResponse, NacelleError>
where
    Req: RequestMetadata + Send + 'static,
    H: Handler,
{
    let request = NacelleRequest {
        meta: NacelleRequestMeta::RawTcp(request.raw_tcp_meta(body_len)),
        body,
    };
    handler.call(request).await
}

async fn pump_request_body<R>(
    reader: &mut R,
    read_buf: &mut BytesMut,
    body_len: usize,
    tx: &mpsc::Sender<Result<Bytes, NacelleError>>,
    config: &NacelleConfig,
) -> Result<(), NacelleError>
where
    R: AsyncRead + Unpin,
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

fn buffered_request_body(
    read_buf: &mut BytesMut,
    body_len: usize,
    chunk_size: usize,
) -> NacelleBody {
    if body_len == 0 {
        return NacelleBody::empty();
    }

    if body_len <= chunk_size {
        let chunk = read_buf.split_to(body_len).freeze();
        return NacelleBody::from_single_chunk(chunk, body_len);
    }

    let mut remaining = body_len;
    let mut chunks = Vec::with_capacity(body_len.div_ceil(chunk_size));
    while remaining > 0 {
        let take = remaining.min(chunk_size);
        chunks.push(read_buf.split_to(take).freeze());
        remaining -= take;
    }

    NacelleBody::from_buffered(chunks, body_len)
}

async fn encode_response_body<Req, P>(
    protocol: &P,
    mut context: P::ResponseContext,
    response: NacelleResponse,
    write_buf: &mut BytesMut,
) -> Result<(), NacelleError>
where
    Req: RequestMetadata,
    P: Protocol<Req> + Send + Sync + 'static,
{
    #[cfg(not(feature = "http"))]
    {
        let NacelleResponseMeta::RawTcp(meta) = &response.meta;
        protocol.apply_raw_tcp_response_meta(&mut context, meta);
    }
    #[cfg(feature = "http")]
    {
        if let NacelleResponseMeta::RawTcp(meta) = &response.meta {
            protocol.apply_raw_tcp_response_meta(&mut context, meta);
        }
    }

    let body = response.body;
    let mut body = match body.try_into_single_chunk_or_empty() {
        Ok(Some(chunk)) => {
            protocol.encode_response_terminal_chunk(&mut context, chunk, write_buf)?;
            return Ok(());
        }
        Ok(None) => {
            protocol.encode_response_end(&mut context, write_buf)?;
            return Ok(());
        }
        Err(body) => body,
    };

    let mut pending_chunk = None;
    while let Some(chunk) = body.next_chunk().await {
        let chunk = chunk?;
        if chunk.is_empty() {
            continue;
        }
        if let Some(prev) = pending_chunk.replace(chunk) {
            protocol.encode_response_chunk(&mut context, prev, write_buf)?;
        }
    }

    if let Some(last) = pending_chunk {
        protocol.encode_response_terminal_chunk(&mut context, last, write_buf)?;
    } else {
        protocol.encode_response_end(&mut context, write_buf)?;
    }

    Ok(())
}

fn write_error<Req, P>(
    write_buf: &mut BytesMut,
    protocol: &P,
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
