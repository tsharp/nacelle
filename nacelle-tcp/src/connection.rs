use std::sync::Arc;

use bytes::{Bytes, BytesMut};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
use tokio::sync::mpsc;

use crate::protocol::{DecodedRequest, Protocol};
use nacelle_core::config::{NacelleConfig, RequestBodyMode};
use nacelle_core::error::NacelleError;
use nacelle_core::handler::Handler;
use nacelle_core::limits::{MemoryReservation, NacelleRuntimeState};
use nacelle_core::request::{
    NacelleBody, NacelleConnectionMeta, NacelleRequest, NacelleRequestMeta, RequestMetadata,
};
use nacelle_core::response::NacelleResponse;
use nacelle_core::telemetry::{NacelleTelemetry, NacelleTransport};

/// Drive one raw TCP framed connection and coalesce completed responses into writes.
pub async fn serve_connection<Req, P, H, R, W>(
    reader: R,
    writer: W,
    protocol: Arc<P>,
    handler: H,
    config: NacelleConfig,
    telemetry: NacelleTelemetry,
    runtime_state: NacelleRuntimeState,
) -> Result<(), NacelleError>
where
    Req: RequestMetadata + Send + 'static,
    P: Protocol<Req> + Send + Sync + 'static,
    H: Handler,
    R: AsyncRead + Unpin + Send + 'static,
    W: AsyncWrite + Unpin + Send + 'static,
{
    serve_connection_with_connection_meta(
        reader,
        writer,
        protocol,
        handler,
        config,
        telemetry,
        runtime_state,
        NacelleConnectionMeta::raw_tcp(None, None),
    )
    .await
}

/// Drive one raw TCP framed connection with caller-supplied connection metadata.
pub async fn serve_connection_with_connection_meta<Req, P, H, R, W>(
    mut reader: R,
    mut writer: W,
    protocol: Arc<P>,
    handler: H,
    config: NacelleConfig,
    telemetry: NacelleTelemetry,
    runtime_state: NacelleRuntimeState,
    connection: NacelleConnectionMeta,
) -> Result<(), NacelleError>
where
    Req: RequestMetadata + Send + 'static,
    P: Protocol<Req> + Send + Sync + 'static,
    H: Handler,
    R: AsyncRead + Unpin + Send + 'static,
    W: AsyncWrite + Unpin + Send + 'static,
{
    let _connection_permit = runtime_state.acquire_connection_tracked()?;
    let _buffer_reservation = reserve_connection_buffers(&config, &runtime_state)?;
    let mut read_buf = BytesMut::with_capacity(config.read_buffer_capacity);
    let mut write_buf = BytesMut::with_capacity(config.response_buffer_capacity);
    telemetry.connection_opened(NacelleTransport::RawTcp);

    let result: Result<(), NacelleError> = async {
        'conn: loop {
            if !write_buf.is_empty() {
                write_all_with_timeout(&mut writer, &write_buf, &runtime_state, "raw_tcp_write")
                    .await?;
                write_buf.clear();
                if write_buf.capacity() > config.response_buffer_capacity {
                    write_buf = BytesMut::with_capacity(config.response_buffer_capacity);
                }
            }

            if read_buf.is_empty() && read_buf.capacity() > config.read_buffer_capacity {
                read_buf = BytesMut::with_capacity(config.read_buffer_capacity);
            }

            let bytes_read =
                read_buf_with_timeout(&mut reader, &mut read_buf, &runtime_state, "raw_tcp_read")
                    .await?;
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
                    &runtime_state,
                    &connection,
                )
                .await?;
            }
        }

        Ok(())
    }
    .await;

    if !write_buf.is_empty() {
        let _ = write_all_with_timeout(
            &mut writer,
            &write_buf,
            &runtime_state,
            "raw_tcp_final_write",
        )
        .await;
    }

    result
}

/// Drive one raw TCP framed connection using a single unsplit I/O object.
pub async fn serve_stream<Req, P, H, IO>(
    io: IO,
    protocol: Arc<P>,
    handler: H,
    config: NacelleConfig,
    telemetry: NacelleTelemetry,
    runtime_state: NacelleRuntimeState,
) -> Result<(), NacelleError>
where
    Req: RequestMetadata + Send + 'static,
    P: Protocol<Req> + Send + Sync + 'static,
    H: Handler,
    IO: AsyncRead + AsyncWrite + Unpin + Send + 'static,
{
    serve_stream_with_connection_meta(
        io,
        protocol,
        handler,
        config,
        telemetry,
        runtime_state,
        NacelleConnectionMeta::raw_tcp(None, None),
    )
    .await
}

/// Drive one raw TCP framed connection using a single unsplit I/O object and caller-supplied metadata.
pub async fn serve_stream_with_connection_meta<Req, P, H, IO>(
    mut io: IO,
    protocol: Arc<P>,
    handler: H,
    config: NacelleConfig,
    telemetry: NacelleTelemetry,
    runtime_state: NacelleRuntimeState,
    connection: NacelleConnectionMeta,
) -> Result<(), NacelleError>
where
    Req: RequestMetadata + Send + 'static,
    P: Protocol<Req> + Send + Sync + 'static,
    H: Handler,
    IO: AsyncRead + AsyncWrite + Unpin + Send + 'static,
{
    let _connection_permit = runtime_state.acquire_connection_tracked()?;
    let _buffer_reservation = reserve_connection_buffers(&config, &runtime_state)?;
    let mut read_buf = BytesMut::with_capacity(config.read_buffer_capacity);
    let mut write_buf = BytesMut::with_capacity(config.response_buffer_capacity);
    telemetry.connection_opened(NacelleTransport::RawTcp);

    let result: Result<(), NacelleError> = async {
        'conn: loop {
            if !write_buf.is_empty() {
                write_all_with_timeout(&mut io, &write_buf, &runtime_state, "raw_tcp_write")
                    .await?;
                write_buf.clear();
                if write_buf.capacity() > config.response_buffer_capacity {
                    write_buf = BytesMut::with_capacity(config.response_buffer_capacity);
                }
            }

            if read_buf.is_empty() && read_buf.capacity() > config.read_buffer_capacity {
                read_buf = BytesMut::with_capacity(config.read_buffer_capacity);
            }

            let bytes_read =
                read_buf_with_timeout(&mut io, &mut read_buf, &runtime_state, "raw_tcp_read")
                    .await?;
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
                    &runtime_state,
                    &connection,
                )
                .await?;
            }
        }

        Ok(())
    }
    .await;

    if !write_buf.is_empty() {
        let _ = write_all_with_timeout(&mut io, &write_buf, &runtime_state, "raw_tcp_final_write")
            .await;
    }

    result
}

/// Drive one raw TCP framed connection using a single unsplit I/O object.
pub async fn serve_stream_without_connection_limit<Req, P, H, IO>(
    io: IO,
    protocol: Arc<P>,
    handler: H,
    config: NacelleConfig,
    telemetry: NacelleTelemetry,
    runtime_state: NacelleRuntimeState,
) -> Result<(), NacelleError>
where
    Req: RequestMetadata + Send + 'static,
    P: Protocol<Req> + Send + Sync + 'static,
    H: Handler,
    IO: AsyncRead + AsyncWrite + Unpin + Send + 'static,
{
    serve_stream_without_connection_limit_with_connection_meta(
        io,
        protocol,
        handler,
        config,
        telemetry,
        runtime_state,
        NacelleConnectionMeta::raw_tcp(None, None),
    )
    .await
}

/// Drive one raw TCP framed connection without taking a connection permit.
pub async fn serve_stream_without_connection_limit_with_connection_meta<Req, P, H, IO>(
    mut io: IO,
    protocol: Arc<P>,
    handler: H,
    config: NacelleConfig,
    telemetry: NacelleTelemetry,
    runtime_state: NacelleRuntimeState,
    connection: NacelleConnectionMeta,
) -> Result<(), NacelleError>
where
    Req: RequestMetadata + Send + 'static,
    P: Protocol<Req> + Send + Sync + 'static,
    H: Handler,
    IO: AsyncRead + AsyncWrite + Unpin + Send + 'static,
{
    let _buffer_reservation = reserve_connection_buffers(&config, &runtime_state)?;
    let mut read_buf = BytesMut::with_capacity(config.read_buffer_capacity);
    let mut write_buf = BytesMut::with_capacity(config.response_buffer_capacity);
    telemetry.connection_opened(NacelleTransport::RawTcp);

    let result: Result<(), NacelleError> = async {
        'conn: loop {
            if !write_buf.is_empty() {
                write_all_with_timeout(&mut io, &write_buf, &runtime_state, "raw_tcp_write")
                    .await?;
                write_buf.clear();
                if write_buf.capacity() > config.response_buffer_capacity {
                    write_buf = BytesMut::with_capacity(config.response_buffer_capacity);
                }
            }

            if read_buf.is_empty() && read_buf.capacity() > config.read_buffer_capacity {
                read_buf = BytesMut::with_capacity(config.read_buffer_capacity);
            }

            let bytes_read =
                read_buf_with_timeout(&mut io, &mut read_buf, &runtime_state, "raw_tcp_read")
                    .await?;
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
                    &runtime_state,
                    &connection,
                )
                .await?;
            }
        }

        Ok(())
    }
    .await;

    if !write_buf.is_empty() {
        let _ = write_all_with_timeout(&mut io, &write_buf, &runtime_state, "raw_tcp_final_write")
            .await;
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
    runtime_state: &NacelleRuntimeState,
    connection: &NacelleConnectionMeta,
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
    if decoded.body_len > runtime_state.limits().max_request_body_bytes {
        let error = NacelleError::ResourceLimit("request_body_bytes");
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
        return Err(NacelleError::ResourceLimit("request_body_bytes"));
    }
    let _request_permit = runtime_state.acquire_request_tracked()?;
    let outcome = if decoded.body_len <= read_buf.len() {
        let body =
            buffered_request_body(read_buf, decoded.body_len, config.request_body_chunk_size);
        execute_handler(
            handler,
            request,
            decoded.body_len,
            body,
            runtime_state,
            connection,
        )
        .await
    } else if config.request_body_mode == RequestBodyMode::Buffered {
        let body =
            read_buffered_request_body(reader, read_buf, decoded.body_len, runtime_state).await?;
        execute_handler(
            handler,
            request,
            decoded.body_len,
            body,
            runtime_state,
            connection,
        )
        .await
    } else {
        let _streaming_permit = runtime_state.acquire_streaming_task_tracked()?;
        let _streaming_body_reservation = runtime_state.reserve_memory(decoded.body_len)?;
        let (body_tx, body_rx) = mpsc::channel(config.request_body_channel_capacity);
        let body = NacelleBody::new(body_rx, decoded.body_len);
        let h = handler.clone();
        let state = runtime_state.clone();
        let connection = connection.clone();
        let handler_task = nacelle_core::runtime::spawn(async move {
            execute_handler(&h, request, decoded.body_len, body, &state, &connection).await
        });

        let pump_result = pump_request_body(
            reader,
            read_buf,
            decoded.body_len,
            &body_tx,
            config,
            runtime_state,
        )
        .await;
        drop(body_tx);
        let outcome = handler_task.await??;
        pump_result?;
        Ok(outcome)
    };

    match outcome {
        Ok(response) => {
            let prev_response_len = write_buf.len();
            encode_response_body::<Req, P>(
                protocol,
                response_context,
                response,
                write_buf,
                runtime_state,
            )
            .await?;
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
    runtime_state: &NacelleRuntimeState,
) -> Result<NacelleBody, NacelleError>
where
    R: AsyncRead + Unpin,
{
    if body_len == 0 {
        return Ok(NacelleBody::empty());
    }

    let reservation = runtime_state.reserve_memory(body_len)?;
    let mut body = BytesMut::with_capacity(body_len);
    if !read_buf.is_empty() {
        let take = body_len.min(read_buf.len());
        body.extend_from_slice(&read_buf.split_to(take));
    }

    while body.len() < body_len {
        let bytes_read =
            read_buf_with_timeout(reader, &mut body, runtime_state, "request_body_read").await?;
        if bytes_read == 0 {
            return Err(NacelleError::UnexpectedEof);
        }
    }

    Ok(
        NacelleBody::from_single_chunk(body.freeze(), body_len)
            .with_memory_reservation(reservation),
    )
}

async fn execute_handler<Req, H>(
    handler: &H,
    request: Req,
    body_len: usize,
    body: NacelleBody,
    runtime_state: &NacelleRuntimeState,
    connection: &NacelleConnectionMeta,
) -> Result<NacelleResponse, NacelleError>
where
    Req: RequestMetadata + Send + 'static,
    H: Handler,
{
    let request = NacelleRequest {
        connection: connection.clone(),
        meta: NacelleRequestMeta::RawTcp(request.raw_tcp_meta(body_len)),
        body,
    };
    let future = handler.call(request);
    if let Some(timeout) = runtime_state.limits().handler_timeout {
        tokio::time::timeout(timeout, future)
            .await
            .map_err(|_| NacelleError::Timeout("handler"))?
    } else {
        future.await
    }
}

async fn pump_request_body<R>(
    reader: &mut R,
    read_buf: &mut BytesMut,
    body_len: usize,
    tx: &mpsc::Sender<Result<Bytes, NacelleError>>,
    config: &NacelleConfig,
    runtime_state: &NacelleRuntimeState,
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
        let bytes_read =
            read_buf_with_timeout(reader, &mut chunk, runtime_state, "request_body_read").await?;
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
    runtime_state: &NacelleRuntimeState,
) -> Result<(), NacelleError>
where
    Req: RequestMetadata,
    P: Protocol<Req> + Send + Sync + 'static,
{
    let Some(meta) = response.meta.raw_tcp() else {
        return Err(NacelleError::InvalidFrame("non_raw_tcp_response"));
    };
    protocol.apply_raw_tcp_response_meta(&mut context, meta);

    let body = response.body;
    let mut response_body_bytes = 0_usize;
    let mut body = match body.try_into_single_chunk_or_empty() {
        Ok(Some(chunk)) => {
            validate_response_bytes(&mut response_body_bytes, chunk.len(), runtime_state)?;
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
        validate_response_bytes(&mut response_body_bytes, chunk.len(), runtime_state)?;
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

fn reserve_connection_buffers(
    config: &NacelleConfig,
    runtime_state: &NacelleRuntimeState,
) -> Result<MemoryReservation, NacelleError> {
    let bytes = config
        .read_buffer_capacity
        .saturating_add(config.response_buffer_capacity);
    runtime_state.reserve_memory(bytes)
}

async fn read_buf_with_timeout<R>(
    reader: &mut R,
    buf: &mut BytesMut,
    runtime_state: &NacelleRuntimeState,
    name: &'static str,
) -> Result<usize, NacelleError>
where
    R: AsyncRead + Unpin,
{
    let future = reader.read_buf(buf);
    if let Some(timeout) = runtime_state
        .limits()
        .read_timeout
        .or(runtime_state.limits().idle_timeout)
    {
        tokio::time::timeout(timeout, future)
            .await
            .map_err(|_| NacelleError::Timeout(name))?
            .map_err(NacelleError::from)
    } else {
        future.await.map_err(NacelleError::from)
    }
}

async fn write_all_with_timeout<W>(
    writer: &mut W,
    buf: &[u8],
    runtime_state: &NacelleRuntimeState,
    name: &'static str,
) -> Result<(), NacelleError>
where
    W: AsyncWrite + Unpin,
{
    let future = writer.write_all(buf);
    if let Some(timeout) = runtime_state.limits().write_timeout {
        tokio::time::timeout(timeout, future)
            .await
            .map_err(|_| NacelleError::Timeout(name))?
            .map_err(NacelleError::from)
    } else {
        future.await.map_err(NacelleError::from)
    }
}

fn validate_response_bytes(
    total: &mut usize,
    next_chunk_len: usize,
    runtime_state: &NacelleRuntimeState,
) -> Result<(), NacelleError> {
    let Some(next) = total.checked_add(next_chunk_len) else {
        return Err(NacelleError::ResourceLimit("response_body_bytes"));
    };
    if next > runtime_state.limits().max_response_body_bytes {
        return Err(NacelleError::ResourceLimit("response_body_bytes"));
    }
    *total = next;
    Ok(())
}
