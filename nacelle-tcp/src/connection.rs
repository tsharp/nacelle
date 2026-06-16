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
use nacelle_core::telemetry::{NacelleTcpMetricsContext, NacelleTelemetry};

/// Drive one TCP framed connection and coalesce completed responses into writes.
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
        NacelleConnectionMeta::tcp(None, None),
    )
    .await
}

/// Drive one TCP framed connection with caller-supplied connection metadata.
#[allow(clippy::too_many_arguments)]
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
    let transport = connection.transport;
    let connection_metrics = tcp_metrics_context(protocol.as_ref(), &connection, None);
    telemetry.tcp_connection_accepted(&connection_metrics);
    telemetry.connection_opened(transport);

    let result: Result<(), NacelleError> = async {
        'conn: loop {
            if !write_buf.is_empty() {
                let write_started = start_tcp_phase(&telemetry);
                let write_result =
                    write_all_with_timeout(&mut writer, &write_buf, &runtime_state, "tcp_write")
                        .await;
                finish_tcp_phase(
                    &telemetry,
                    &connection_metrics,
                    "socket_write",
                    write_started,
                );
                if let Err(error) = write_result {
                    telemetry.tcp_error(&connection_metrics, "socket_write", &error);
                    return Err(error);
                }
                write_buf.clear();
                if write_buf.capacity() > config.response_buffer_capacity {
                    write_buf = BytesMut::with_capacity(config.response_buffer_capacity);
                }
            }

            if read_buf.is_empty() && read_buf.capacity() > config.read_buffer_capacity {
                read_buf = BytesMut::with_capacity(config.read_buffer_capacity);
            }

            let read_started = start_tcp_phase(&telemetry);
            let read_result =
                read_buf_with_timeout(&mut reader, &mut read_buf, &runtime_state, "tcp_read").await;
            finish_tcp_phase(&telemetry, &connection_metrics, "socket_read", read_started);
            let bytes_read = match read_result {
                Ok(bytes_read) => bytes_read,
                Err(error) => {
                    telemetry.tcp_error(&connection_metrics, "socket_read", &error);
                    return Err(error);
                }
            };
            if bytes_read == 0 {
                if read_buf.is_empty() {
                    break 'conn;
                }
                return Err(NacelleError::UnexpectedEof);
            }

            while let Some(decoded) = decode_next_request(
                protocol.as_ref(),
                &mut read_buf,
                config.max_frame_len,
                &telemetry,
                &connection_metrics,
            )? {
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
        let write_started = start_tcp_phase(&telemetry);
        let final_write =
            write_all_with_timeout(&mut writer, &write_buf, &runtime_state, "tcp_final_write")
                .await;
        finish_tcp_phase(
            &telemetry,
            &connection_metrics,
            "socket_write",
            write_started,
        );
        if let Err(error) = &final_write {
            telemetry.tcp_error(&connection_metrics, "socket_write", error);
        }
    }

    telemetry.tcp_connection_closed(&connection_metrics, tcp_close_reason(&result));
    result
}

/// Drive one TCP framed connection using a single unsplit I/O object.
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
        NacelleConnectionMeta::tcp(None, None),
    )
    .await
}

/// Drive one TCP framed connection using a single unsplit I/O object and caller-supplied metadata.
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
    let transport = connection.transport;
    let connection_metrics = tcp_metrics_context(protocol.as_ref(), &connection, None);
    telemetry.tcp_connection_accepted(&connection_metrics);
    telemetry.connection_opened(transport);

    let result: Result<(), NacelleError> = async {
        'conn: loop {
            if !write_buf.is_empty() {
                let write_started = start_tcp_phase(&telemetry);
                let write_result =
                    write_all_with_timeout(&mut io, &write_buf, &runtime_state, "tcp_write").await;
                finish_tcp_phase(
                    &telemetry,
                    &connection_metrics,
                    "socket_write",
                    write_started,
                );
                if let Err(error) = write_result {
                    telemetry.tcp_error(&connection_metrics, "socket_write", &error);
                    return Err(error);
                }
                write_buf.clear();
                if write_buf.capacity() > config.response_buffer_capacity {
                    write_buf = BytesMut::with_capacity(config.response_buffer_capacity);
                }
            }

            if read_buf.is_empty() && read_buf.capacity() > config.read_buffer_capacity {
                read_buf = BytesMut::with_capacity(config.read_buffer_capacity);
            }

            let read_started = start_tcp_phase(&telemetry);
            let read_result =
                read_buf_with_timeout(&mut io, &mut read_buf, &runtime_state, "tcp_read").await;
            finish_tcp_phase(&telemetry, &connection_metrics, "socket_read", read_started);
            let bytes_read = match read_result {
                Ok(bytes_read) => bytes_read,
                Err(error) => {
                    telemetry.tcp_error(&connection_metrics, "socket_read", &error);
                    return Err(error);
                }
            };
            if bytes_read == 0 {
                if read_buf.is_empty() {
                    break 'conn;
                }
                return Err(NacelleError::UnexpectedEof);
            }

            while let Some(decoded) = decode_next_request(
                protocol.as_ref(),
                &mut read_buf,
                config.max_frame_len,
                &telemetry,
                &connection_metrics,
            )? {
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
        let write_started = start_tcp_phase(&telemetry);
        let final_write =
            write_all_with_timeout(&mut io, &write_buf, &runtime_state, "tcp_final_write").await;
        finish_tcp_phase(
            &telemetry,
            &connection_metrics,
            "socket_write",
            write_started,
        );
        if let Err(error) = &final_write {
            telemetry.tcp_error(&connection_metrics, "socket_write", error);
        }
    }

    telemetry.tcp_connection_closed(&connection_metrics, tcp_close_reason(&result));
    result
}

/// Drive one TCP framed connection using a single unsplit I/O object.
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
        NacelleConnectionMeta::tcp(None, None),
    )
    .await
}

/// Drive one TCP framed connection without taking a connection permit.
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
    let transport = connection.transport;
    let connection_metrics = tcp_metrics_context(protocol.as_ref(), &connection, None);
    telemetry.tcp_connection_accepted(&connection_metrics);
    telemetry.connection_opened(transport);

    let result: Result<(), NacelleError> = async {
        'conn: loop {
            if !write_buf.is_empty() {
                let write_started = start_tcp_phase(&telemetry);
                let write_result =
                    write_all_with_timeout(&mut io, &write_buf, &runtime_state, "tcp_write").await;
                finish_tcp_phase(
                    &telemetry,
                    &connection_metrics,
                    "socket_write",
                    write_started,
                );
                if let Err(error) = write_result {
                    telemetry.tcp_error(&connection_metrics, "socket_write", &error);
                    return Err(error);
                }
                write_buf.clear();
                if write_buf.capacity() > config.response_buffer_capacity {
                    write_buf = BytesMut::with_capacity(config.response_buffer_capacity);
                }
            }

            if read_buf.is_empty() && read_buf.capacity() > config.read_buffer_capacity {
                read_buf = BytesMut::with_capacity(config.read_buffer_capacity);
            }

            let read_started = start_tcp_phase(&telemetry);
            let read_result =
                read_buf_with_timeout(&mut io, &mut read_buf, &runtime_state, "tcp_read").await;
            finish_tcp_phase(&telemetry, &connection_metrics, "socket_read", read_started);
            let bytes_read = match read_result {
                Ok(bytes_read) => bytes_read,
                Err(error) => {
                    telemetry.tcp_error(&connection_metrics, "socket_read", &error);
                    return Err(error);
                }
            };
            if bytes_read == 0 {
                if read_buf.is_empty() {
                    break 'conn;
                }
                return Err(NacelleError::UnexpectedEof);
            }

            while let Some(decoded) = decode_next_request(
                protocol.as_ref(),
                &mut read_buf,
                config.max_frame_len,
                &telemetry,
                &connection_metrics,
            )? {
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
        let write_started = start_tcp_phase(&telemetry);
        let final_write =
            write_all_with_timeout(&mut io, &write_buf, &runtime_state, "tcp_final_write").await;
        finish_tcp_phase(
            &telemetry,
            &connection_metrics,
            "socket_write",
            write_started,
        );
        if let Err(error) = &final_write {
            telemetry.tcp_error(&connection_metrics, "socket_write", error);
        }
    }

    telemetry.tcp_connection_closed(&connection_metrics, tcp_close_reason(&result));
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
    let metrics_context = tcp_metrics_context(protocol, connection, Some(opcode));
    let request_bytes = 4 + 20 + decoded.body_len;
    let response_context = protocol.response_context(&request);
    let max_request_body_bytes =
        request.max_body_bytes(connection, runtime_state.limits().max_request_body_bytes);
    if decoded.body_len > max_request_body_bytes {
        let error = NacelleError::ResourceLimit("request_body_bytes");
        telemetry.tcp_error(&metrics_context, "request_body_limit", &error);
        telemetry.request_failed(
            connection.transport,
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
    let _request_permit = match runtime_state.acquire_request_tracked() {
        Ok(permit) => permit,
        Err(error) => {
            telemetry.tcp_error(&metrics_context, "request_permit", &error);
            return Err(error);
        }
    };
    let mut request_metrics = TcpRequestMetricsGuard::new(
        telemetry,
        metrics_context.clone(),
        request_bytes,
        decoded.body_len,
        request_started,
    );
    let outcome = if decoded.body_len <= read_buf.len() {
        let body_started = start_tcp_phase(telemetry);
        let body =
            buffered_request_body(read_buf, decoded.body_len, config.request_body_chunk_size);
        finish_tcp_phase(
            telemetry,
            &metrics_context,
            "request_body_read",
            body_started,
        );
        execute_handler_with_metrics(
            handler,
            request,
            decoded.body_len,
            body,
            runtime_state,
            connection,
            telemetry,
            &metrics_context,
        )
        .await
    } else if config.request_body_mode == RequestBodyMode::Buffered {
        let body_started = start_tcp_phase(telemetry);
        let body = read_buffered_request_body(reader, read_buf, decoded.body_len, runtime_state)
            .await
            .inspect_err(|error| {
                telemetry.tcp_error(&metrics_context, "request_body_read", error)
            })?;
        finish_tcp_phase(
            telemetry,
            &metrics_context,
            "request_body_read",
            body_started,
        );
        execute_handler_with_metrics(
            handler,
            request,
            decoded.body_len,
            body,
            runtime_state,
            connection,
            telemetry,
            &metrics_context,
        )
        .await
    } else {
        let _streaming_permit = runtime_state
            .acquire_streaming_task_tracked()
            .inspect_err(|error| telemetry.tcp_error(&metrics_context, "streaming_task", error))?;
        let _streaming_body_reservation = runtime_state
            .reserve_memory(decoded.body_len)
            .inspect_err(|error| {
                telemetry.tcp_error(&metrics_context, "streaming_memory", error)
            })?;
        let (body_tx, body_rx) = mpsc::channel(config.request_body_channel_capacity);
        let body = NacelleBody::new(body_rx, decoded.body_len);
        let h = handler.clone();
        let state = runtime_state.clone();
        let connection = connection.clone();
        let handler_telemetry = telemetry.clone();
        let handler_metrics_context = metrics_context.clone();
        let handler_task = nacelle_core::runtime::spawn(async move {
            execute_handler_with_metrics(
                &h,
                request,
                decoded.body_len,
                body,
                &state,
                &connection,
                &handler_telemetry,
                &handler_metrics_context,
            )
            .await
        });

        let body_started = start_tcp_phase(telemetry);
        let pump_result = pump_request_body(
            reader,
            read_buf,
            decoded.body_len,
            &body_tx,
            config,
            runtime_state,
        )
        .await;
        finish_tcp_phase(
            telemetry,
            &metrics_context,
            "request_body_read",
            body_started,
        );
        if let Err(error) = &pump_result {
            telemetry.tcp_error(&metrics_context, "request_body_read", error);
        }
        drop(body_tx);
        let outcome = match handler_task.await {
            Ok(outcome) => outcome?,
            Err(error) => {
                let error = NacelleError::from(error);
                telemetry.tcp_error(&metrics_context, "handler_join", &error);
                return Err(error);
            }
        };
        pump_result?;
        Ok(outcome)
    };

    match outcome {
        Ok(response) => {
            let prev_response_len = write_buf.len();
            let encode_started = start_tcp_phase(telemetry);
            let encode_result = encode_response_body::<Req, P>(
                protocol,
                response_context,
                response,
                write_buf,
                runtime_state,
            )
            .await;
            finish_tcp_phase(
                telemetry,
                &metrics_context,
                "response_encode",
                encode_started,
            );
            if let Err(error) = encode_result {
                telemetry.tcp_error(&metrics_context, "response_encode", &error);
                return Err(error);
            }
            let response_bytes = write_buf.len().saturating_sub(prev_response_len);
            telemetry.request_completed(
                connection.transport,
                Some(opcode),
                request_bytes,
                response_bytes,
                request_started.elapsed(),
            );
            request_metrics.complete("ok", response_bytes);
        }
        Err(error) => {
            let prev_response_len = write_buf.len();
            telemetry.tcp_error(&metrics_context, "handler", &error);
            telemetry.request_failed(
                connection.transport,
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
            let response_bytes = write_buf.len().saturating_sub(prev_response_len);
            telemetry.request_completed(
                connection.transport,
                Some(opcode),
                request_bytes,
                response_bytes,
                request_started.elapsed(),
            );
            request_metrics.complete("error", response_bytes);
        }
    }

    Ok(())
}

#[allow(clippy::too_many_arguments)]
async fn execute_handler_with_metrics<Req, H>(
    handler: &H,
    request: Req,
    body_len: usize,
    body: NacelleBody,
    runtime_state: &NacelleRuntimeState,
    connection: &NacelleConnectionMeta,
    telemetry: &NacelleTelemetry,
    metrics_context: &NacelleTcpMetricsContext,
) -> Result<NacelleResponse, NacelleError>
where
    Req: RequestMetadata + Send + 'static,
    H: Handler,
{
    let handler_started = start_tcp_phase(telemetry);
    let result = execute_handler(handler, request, body_len, body, runtime_state, connection).await;
    finish_tcp_phase(telemetry, metrics_context, "handler", handler_started);
    result
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
        meta: NacelleRequestMeta::Tcp(request.tcp_meta(body_len)),
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
    let Some(meta) = response.meta.tcp() else {
        return Err(NacelleError::InvalidFrame("non_tcp_response"));
    };
    protocol.apply_tcp_response_meta(&mut context, meta);

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

fn decode_next_request<Req, P>(
    protocol: &P,
    read_buf: &mut BytesMut,
    max_frame_len: usize,
    telemetry: &NacelleTelemetry,
    metrics_context: &NacelleTcpMetricsContext,
) -> Result<Option<DecodedRequest<Req>>, NacelleError>
where
    Req: RequestMetadata + Send + 'static,
    P: Protocol<Req> + Send + Sync + 'static,
{
    let decode_started = start_tcp_phase(telemetry);
    let result = protocol.decode_head(read_buf, max_frame_len);
    finish_tcp_phase(telemetry, metrics_context, "decode", decode_started);
    if let Err(error) = &result {
        telemetry.tcp_error(metrics_context, "decode", error);
    }
    result
}

fn tcp_metrics_context<Req, P>(
    protocol: &P,
    connection: &NacelleConnectionMeta,
    opcode: Option<u64>,
) -> NacelleTcpMetricsContext
where
    Req: RequestMetadata,
    P: Protocol<Req> + Send + Sync + 'static,
{
    NacelleTcpMetricsContext::new(
        connection.transport,
        connection.listener.clone(),
        protocol.name(),
        connection.tls_label(),
        opcode,
    )
}

fn start_tcp_phase(telemetry: &NacelleTelemetry) -> Option<std::time::Instant> {
    telemetry
        .tcp_metrics_enabled()
        .then(std::time::Instant::now)
}

fn finish_tcp_phase(
    telemetry: &NacelleTelemetry,
    metrics_context: &NacelleTcpMetricsContext,
    phase: &'static str,
    started: Option<std::time::Instant>,
) {
    if let Some(started) = started {
        telemetry.tcp_phase_duration(metrics_context, phase, started.elapsed());
    }
}

fn tcp_close_reason(result: &Result<(), NacelleError>) -> &'static str {
    match result {
        Ok(()) => "eof",
        Err(NacelleError::Timeout(_)) => "timeout",
        Err(NacelleError::UnexpectedEof) => "unexpected_eof",
        Err(NacelleError::ConnectionClosed) => "connection_closed",
        Err(NacelleError::ResourceLimit(_)) => "resource_limit",
        Err(NacelleError::Io(_)) => "io",
        Err(NacelleError::Protocol(_)) => "protocol",
        Err(NacelleError::Handler(_)) => "handler",
        Err(NacelleError::Join(_)) => "join",
        Err(NacelleError::InvalidFrame(_)) | Err(NacelleError::FrameTooLarge { .. }) => {
            "invalid_frame"
        }
        Err(NacelleError::MissingProtocol) => "missing_protocol",
    }
}

struct TcpRequestMetricsGuard<'a> {
    telemetry: &'a NacelleTelemetry,
    context: NacelleTcpMetricsContext,
    request_bytes: usize,
    started: std::time::Instant,
    completed: bool,
}

impl<'a> TcpRequestMetricsGuard<'a> {
    fn new(
        telemetry: &'a NacelleTelemetry,
        context: NacelleTcpMetricsContext,
        request_bytes: usize,
        body_bytes: usize,
        started: std::time::Instant,
    ) -> Self {
        telemetry.tcp_request_started(&context);
        telemetry.tcp_request_body_bytes(&context, body_bytes);
        Self {
            telemetry,
            context,
            request_bytes,
            started,
            completed: false,
        }
    }

    fn complete(&mut self, status: &'static str, response_bytes: usize) {
        self.telemetry.tcp_request_completed(
            &self.context,
            status,
            self.request_bytes,
            response_bytes,
            self.started.elapsed(),
        );
        self.completed = true;
    }
}

impl Drop for TcpRequestMetricsGuard<'_> {
    fn drop(&mut self) {
        if !self.completed {
            self.telemetry.tcp_request_completed(
                &self.context,
                "error",
                self.request_bytes,
                0,
                self.started.elapsed(),
            );
        }
    }
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

#[cfg(test)]
mod tests {
    use std::sync::Arc;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::time::Duration;

    use bytes::{Bytes, BytesMut};
    use nacelle_core::handler::handler_fn;
    use nacelle_core::limits::{NacelleLimits, NacelleRuntimeState};
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    use super::*;

    const PRE_AUTH_BODY_LIMIT: usize = 2;

    struct AuthState {
        authenticated: AtomicBool,
    }

    #[derive(Debug)]
    struct PhaseRequest {
        body_len: usize,
    }

    impl RequestMetadata for PhaseRequest {
        fn opcode(&self) -> u64 {
            1
        }

        fn max_body_bytes(
            &self,
            connection: &NacelleConnectionMeta,
            default_limit: usize,
        ) -> usize {
            let authenticated = connection
                .extension::<AuthState>()
                .is_some_and(|state| state.authenticated.load(Ordering::SeqCst));
            if authenticated {
                default_limit
            } else {
                PRE_AUTH_BODY_LIMIT
            }
        }

        fn tcp_meta(&self, _body_len: usize) -> nacelle_core::request::TcpRequestMeta {
            nacelle_core::request::TcpRequestMeta {
                request_id: None,
                opcode: 1,
                flags: 0,
                body_len: self.body_len,
            }
        }
    }

    struct PhaseProtocol;

    impl Protocol<PhaseRequest> for PhaseProtocol {
        type ResponseContext = ();
        type ErrorContext = ();

        fn decode_head(
            &self,
            src: &mut BytesMut,
            _max_frame_len: usize,
        ) -> Result<Option<DecodedRequest<PhaseRequest>>, NacelleError> {
            if src.is_empty() {
                return Ok(None);
            }
            let body_len = src[0] as usize;
            let _head = src.split_to(1);
            Ok(Some(DecodedRequest {
                request: PhaseRequest { body_len },
                body_len,
            }))
        }

        fn response_context(&self, _req: &PhaseRequest) -> Self::ResponseContext {}

        fn error_context(&self, _req: &PhaseRequest) -> Self::ErrorContext {}

        fn encode_response_chunk(
            &self,
            _context: &mut Self::ResponseContext,
            chunk: Bytes,
            dst: &mut BytesMut,
        ) -> Result<(), NacelleError> {
            dst.extend_from_slice(&chunk);
            Ok(())
        }

        fn encode_response_end(
            &self,
            _context: &mut Self::ResponseContext,
            _dst: &mut BytesMut,
        ) -> Result<(), NacelleError> {
            Ok(())
        }

        fn encode_error(
            &self,
            _context: Option<&Self::ErrorContext>,
            error: &NacelleError,
            dst: &mut BytesMut,
        ) -> Result<(), NacelleError> {
            dst.extend_from_slice(error.to_string().as_bytes());
            Ok(())
        }
    }

    #[tokio::test]
    async fn phase_limit_rejects_unauthenticated_body_before_reading_body() {
        let (mut client, server_io) = tokio::io::duplex(1024);
        let handler_called = Arc::new(AtomicBool::new(false));
        let server_task = tokio::spawn(serve_stream_with_connection_meta(
            server_io,
            Arc::new(PhaseProtocol),
            handler_fn({
                let handler_called = handler_called.clone();
                move |_request: NacelleRequest| {
                    handler_called.store(true, Ordering::SeqCst);
                    async move { Ok(NacelleResponse::empty_tcp()) }
                }
            }),
            NacelleConfig::default(),
            NacelleTelemetry::default(),
            NacelleRuntimeState::new(NacelleLimits::default().with_max_request_body_bytes(4)),
            NacelleConnectionMeta::tcp(None, None).with_extension(AuthState {
                authenticated: AtomicBool::new(false),
            }),
        ));

        client.write_all(&[3]).await.expect("head should write");
        let mut response = [0_u8; 128];
        let bytes_read = tokio::time::timeout(Duration::from_secs(1), client.read(&mut response))
            .await
            .expect("response should arrive")
            .expect("response should read");
        let response = std::str::from_utf8(&response[..bytes_read]).expect("utf8 error response");

        assert!(response.contains("request_body_bytes"));
        assert!(!handler_called.load(Ordering::SeqCst));
        let result = tokio::time::timeout(Duration::from_secs(1), server_task)
            .await
            .expect("server should finish")
            .expect("server task should join");
        assert!(matches!(
            result,
            Err(NacelleError::ResourceLimit("request_body_bytes"))
        ));
    }

    #[tokio::test]
    async fn authenticated_phase_uses_default_request_body_limit() {
        let (mut client, server_io) = tokio::io::duplex(1024);
        let handler_called = Arc::new(AtomicBool::new(false));
        let server_task = tokio::spawn(serve_stream_with_connection_meta(
            server_io,
            Arc::new(PhaseProtocol),
            handler_fn({
                let handler_called = handler_called.clone();
                move |mut request: NacelleRequest| {
                    let handler_called = handler_called.clone();
                    async move {
                        handler_called.store(true, Ordering::SeqCst);
                        let mut body = Vec::new();
                        while let Some(chunk) = request.body.next_chunk().await {
                            body.extend_from_slice(&chunk?);
                        }
                        assert_eq!(body, b"hey");
                        Ok(NacelleResponse::tcp_bytes("ok"))
                    }
                }
            }),
            NacelleConfig::default(),
            NacelleTelemetry::default(),
            NacelleRuntimeState::new(NacelleLimits::default().with_max_request_body_bytes(4)),
            NacelleConnectionMeta::tcp(None, None).with_extension(AuthState {
                authenticated: AtomicBool::new(true),
            }),
        ));

        client
            .write_all(&[3, b'h', b'e', b'y'])
            .await
            .expect("request should write");
        let mut response = [0_u8; 2];
        tokio::time::timeout(Duration::from_secs(1), client.read_exact(&mut response))
            .await
            .expect("response should arrive")
            .expect("response should read");
        assert_eq!(&response, b"ok");
        assert!(handler_called.load(Ordering::SeqCst));

        drop(client);
        let result = tokio::time::timeout(Duration::from_secs(1), server_task)
            .await
            .expect("server should finish")
            .expect("server task should join");
        assert!(result.is_ok());
    }
}
