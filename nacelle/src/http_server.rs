use std::convert::Infallible;
use std::io;
use std::net::SocketAddr;
use std::pin::Pin;
use std::sync::Arc;
use std::task::{Context, Poll};
use std::time::Duration;

use bytes::Bytes;
use futures_core::Stream;
use http_body_util::{BodyExt, Full, StreamBody, combinators::BoxBody};
use hyper::body::{Frame, Incoming};
use hyper::server::conn::http1;
use hyper::service::service_fn;
use hyper::{Request, Response, StatusCode};
use hyper_util::rt::{TokioIo, TokioTimer};

use crate::error::{BoxError, NacelleError};
use crate::handler::Handler;
use crate::lifecycle::{NacelleDrainDeadline, NacelleShutdownToken};
use crate::limits::NacelleRuntimeState;
use crate::request::{HttpRequestMeta, NacelleBody, NacelleRequest, NacelleRequestMeta};
use crate::response::{NacelleResponse, NacelleResponseMeta};
use crate::telemetry::{NacelleTelemetry, NacelleTelemetryEventKind, NacelleTransport};

type HttpBody = BoxBody<Bytes, BoxError>;

pub struct HyperServer<H = ()> {
    handler: H,
    telemetry: NacelleTelemetry,
    runtime_state: NacelleRuntimeState,
}

impl<H> Clone for HyperServer<H>
where
    H: Clone,
{
    fn clone(&self) -> Self {
        Self {
            handler: self.handler.clone(),
            telemetry: self.telemetry.clone(),
            runtime_state: self.runtime_state.clone(),
        }
    }
}

impl<H> HyperServer<H>
where
    H: Handler,
{
    pub fn new(handler: H) -> Self {
        Self {
            handler,
            telemetry: NacelleTelemetry::default(),
            runtime_state: NacelleRuntimeState::default(),
        }
    }

    pub fn with_telemetry(mut self, telemetry: NacelleTelemetry) -> Self {
        self.telemetry = telemetry;
        self
    }

    pub fn with_runtime_state(mut self, runtime_state: NacelleRuntimeState) -> Self {
        self.runtime_state = runtime_state;
        self
    }

    pub async fn serve(self, addr: SocketAddr) -> Result<(), NacelleError> {
        let listener = tokio::net::TcpListener::bind(addr).await?;
        self.serve_listener(listener).await
    }

    pub async fn serve_with_shutdown(
        self,
        addr: SocketAddr,
        shutdown: NacelleShutdownToken,
    ) -> Result<(), NacelleError> {
        self.serve_with_shutdown_timeout(addr, shutdown, Duration::from_secs(30))
            .await
    }

    pub async fn serve_with_shutdown_timeout(
        self,
        addr: SocketAddr,
        shutdown: NacelleShutdownToken,
        drain_timeout: Duration,
    ) -> Result<(), NacelleError> {
        self.serve_with_shutdown_deadline(addr, shutdown, NacelleDrainDeadline::new(drain_timeout))
            .await
    }

    pub(crate) async fn serve_with_shutdown_deadline(
        self,
        addr: SocketAddr,
        shutdown: NacelleShutdownToken,
        drain_deadline: NacelleDrainDeadline,
    ) -> Result<(), NacelleError> {
        let listener = tokio::net::TcpListener::bind(addr).await?;
        self.serve_listener_with_shutdown_deadline(listener, shutdown, drain_deadline)
            .await
    }

    pub async fn serve_listener(
        self,
        listener: tokio::net::TcpListener,
    ) -> Result<(), NacelleError> {
        let (_shutdown, token) = crate::lifecycle::NacelleShutdown::pair();
        self.serve_listener_with_shutdown(listener, token).await
    }

    pub async fn serve_listener_with_shutdown(
        self,
        listener: tokio::net::TcpListener,
        shutdown: NacelleShutdownToken,
    ) -> Result<(), NacelleError> {
        self.serve_listener_with_shutdown_timeout(listener, shutdown, Duration::from_secs(30))
            .await
    }

    pub async fn serve_listener_with_shutdown_timeout(
        self,
        listener: tokio::net::TcpListener,
        shutdown: NacelleShutdownToken,
        drain_timeout: Duration,
    ) -> Result<(), NacelleError> {
        self.serve_listener_with_shutdown_deadline(
            listener,
            shutdown,
            NacelleDrainDeadline::new(drain_timeout),
        )
        .await
    }

    pub(crate) async fn serve_listener_with_shutdown_deadline(
        self,
        listener: tokio::net::TcpListener,
        mut shutdown: NacelleShutdownToken,
        drain_deadline: NacelleDrainDeadline,
    ) -> Result<(), NacelleError> {
        let server = Arc::new(self);
        let mut connections = tokio::task::JoinSet::new();
        loop {
            tokio::select! {
                biased;
                _ = shutdown.changed() => break,
                joined = connections.join_next(), if !connections.is_empty() => {
                    log_http_connection_result(joined);
                    continue;
                }
                accepted = listener.accept() => {
                    let (stream, _) = accepted?;
            let server = server.clone();
            let connection_permit = match server.runtime_state.acquire_connection_tracked() {
                Ok(permit) => permit,
                Err(_error) => {
                    server
                        .telemetry
                        .connection_rejected(NacelleTransport::Http, "connections");
                    continue;
                }
            };
            server.telemetry.connection_opened(NacelleTransport::Http);
                    connections.spawn(async move {
                let _connection_permit = connection_permit;
                let write_timeout = server
                    .runtime_state
                    .limits()
                    .http_response_write_timeout
                    .or(server.runtime_state.limits().write_timeout);
                let io = TimeoutIo::new(TokioIo::new(stream), write_timeout);
                let service_server = server.clone();
                let service = service_fn(move |request| {
                    let server = service_server.clone();
                    async move { server.handle(request).await }
                });
                let mut builder = http1::Builder::new();
                builder
                    .timer(TokioTimer::new())
                    .header_read_timeout(server.runtime_state.limits().http_header_read_timeout)
                    .keep_alive(server.runtime_state.limits().http_keep_alive);
                let connection = builder.serve_connection(io, service);
                if let Some(max_age) = server.runtime_state.limits().http_max_connection_age {
                    match tokio::time::timeout(max_age, connection).await {
                        Ok(result) => result.map_err(NacelleError::protocol),
                        Err(_) => {
                            server
                                .telemetry
                                .timeout(NacelleTransport::Http, "http_max_connection_age");
                            Err(NacelleError::Timeout("http_max_connection_age"))
                        }
                    }
                } else {
                    connection.await.map_err(NacelleError::protocol)
                }
            });
                }
            }
        }
        server.telemetry.shutdown_event(
            NacelleTelemetryEventKind::ListenerStoppedAccepting,
            NacelleTransport::Http,
        );
        drain_http_connection_tasks(connections, drain_deadline.get(), server.telemetry.clone())
            .await;
        Ok(())
    }

    async fn handle(&self, request: Request<Incoming>) -> Result<Response<HttpBody>, NacelleError> {
        let request_started = std::time::Instant::now();
        let _request_permit = match self.runtime_state.acquire_request_tracked() {
            Ok(permit) => permit,
            Err(error) => {
                self.telemetry.request_failed(
                    NacelleTransport::Http,
                    None,
                    request_started.elapsed(),
                    &error,
                );
                return response_to_http(
                    NacelleResponse::http_bytes(StatusCode::SERVICE_UNAVAILABLE, error.to_string()),
                    self.runtime_state.clone(),
                );
            }
        };
        let (parts, body) = request.into_parts();
        let request = NacelleRequest {
            meta: NacelleRequestMeta::Http(HttpRequestMeta {
                method: parts.method,
                uri: parts.uri,
                headers: parts.headers,
            }),
            body: incoming_to_body(body, self.runtime_state.clone(), self.telemetry.clone()),
        };

        let handler_future = self.handler.call(request);
        let handler_result = if let Some(timeout) = self.runtime_state.limits().handler_timeout {
            tokio::time::timeout(timeout, handler_future)
                .await
                .map_err(|_| NacelleError::Timeout("handler"))?
        } else {
            handler_future.await
        };

        match handler_result {
            Ok(response) => {
                let response = response_to_http(response, self.runtime_state.clone());
                self.telemetry.request_completed(
                    NacelleTransport::Http,
                    None,
                    0,
                    0,
                    request_started.elapsed(),
                );
                response
            }
            Err(error) => {
                self.telemetry.request_failed(
                    NacelleTransport::Http,
                    None,
                    request_started.elapsed(),
                    &error,
                );
                let response = response_to_http(
                    NacelleResponse::http_bytes(
                        StatusCode::INTERNAL_SERVER_ERROR,
                        error.to_string(),
                    ),
                    self.runtime_state.clone(),
                );
                self.telemetry.request_completed(
                    NacelleTransport::Http,
                    None,
                    0,
                    0,
                    request_started.elapsed(),
                );
                response
            }
        }
    }
}

fn incoming_to_body(
    mut incoming: Incoming,
    runtime_state: NacelleRuntimeState,
    telemetry: NacelleTelemetry,
) -> NacelleBody {
    let (tx, body) = NacelleBody::channel(8);
    tokio::spawn(async move {
        let _streaming_permit = match runtime_state.acquire_streaming_task_tracked() {
            Ok(permit) => permit,
            Err(error) => {
                let _ = tx.send(Err(error)).await;
                return;
            }
        };
        let mut body_bytes = 0_usize;
        loop {
            let frame = {
                let next = incoming.frame();
                if let Some(timeout) = runtime_state
                    .limits()
                    .http_request_body_read_timeout
                    .or(runtime_state.limits().read_timeout)
                {
                    match tokio::time::timeout(timeout, next).await {
                        Ok(frame) => frame,
                        Err(_) => {
                            telemetry.timeout(NacelleTransport::Http, "http_body_read");
                            let _ = tx.send(Err(NacelleError::Timeout("http_body_read"))).await;
                            break;
                        }
                    }
                } else {
                    next.await
                }
            };
            let Some(frame) = frame else {
                break;
            };
            match frame {
                Ok(frame) => {
                    if let Some(data) = frame.data_ref() {
                        let Some(next) = body_bytes.checked_add(data.len()) else {
                            let _ = tx
                                .send(Err(NacelleError::ResourceLimit("request_body_bytes")))
                                .await;
                            break;
                        };
                        if next > runtime_state.limits().max_request_body_bytes {
                            let _ = tx
                                .send(Err(NacelleError::ResourceLimit("request_body_bytes")))
                                .await;
                            break;
                        }
                        body_bytes = next;
                        if tx.send(Ok(data.clone())).await.is_err() {
                            break;
                        }
                    }
                }
                Err(error) => {
                    let _ = tx.send(Err(NacelleError::protocol(error))).await;
                    break;
                }
            }
        }
    });
    body
}

fn response_to_http(
    response: NacelleResponse,
    runtime_state: NacelleRuntimeState,
) -> Result<Response<HttpBody>, NacelleError> {
    let (status, headers) = match response.meta {
        NacelleResponseMeta::Http(meta) => (meta.status, meta.headers),
        NacelleResponseMeta::RawTcp(_) => (StatusCode::OK, http::HeaderMap::new()),
    };

    let mut builder = Response::builder().status(status);
    let Some(builder_headers) = builder.headers_mut() else {
        return Err(NacelleError::protocol("failed to build response headers"));
    };
    *builder_headers = headers;
    builder
        .body(nacelle_body_to_http(response.body, runtime_state))
        .map_err(NacelleError::protocol)
}

fn nacelle_body_to_http(body: NacelleBody, runtime_state: NacelleRuntimeState) -> HttpBody {
    StreamBody::new(HttpBodyStream {
        body,
        runtime_state,
        response_body_bytes: 0,
    })
    .map_err(|error| -> BoxError { Box::new(error) })
    .boxed()
}

struct TimeoutIo<I> {
    inner: I,
    write_timeout: Option<Duration>,
    write_sleep: Option<Pin<Box<tokio::time::Sleep>>>,
}

impl<I> TimeoutIo<I> {
    fn new(inner: I, write_timeout: Option<Duration>) -> Self {
        Self {
            inner,
            write_timeout,
            write_sleep: None,
        }
    }

    fn poll_write_deadline(&mut self, cx: &mut Context<'_>) -> Poll<Result<(), io::Error>> {
        let Some(timeout) = self.write_timeout else {
            return Poll::Pending;
        };
        let sleep = self
            .write_sleep
            .get_or_insert_with(|| Box::pin(tokio::time::sleep(timeout)));
        match sleep.as_mut().poll(cx) {
            Poll::Ready(()) => {
                self.write_sleep = None;
                Poll::Ready(Err(io::Error::new(
                    io::ErrorKind::TimedOut,
                    "http response write timed out",
                )))
            }
            Poll::Pending => Poll::Pending,
        }
    }
}

impl<I> hyper::rt::Read for TimeoutIo<I>
where
    I: hyper::rt::Read + Unpin,
{
    fn poll_read(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: hyper::rt::ReadBufCursor<'_>,
    ) -> Poll<Result<(), io::Error>> {
        Pin::new(&mut self.inner).poll_read(cx, buf)
    }
}

impl<I> hyper::rt::Write for TimeoutIo<I>
where
    I: hyper::rt::Write + Unpin,
{
    fn poll_write(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<Result<usize, io::Error>> {
        if self.write_sleep.is_some() {
            if let Poll::Ready(result) = self.poll_write_deadline(cx) {
                return Poll::Ready(result.map(|()| 0));
            }
        }
        match Pin::new(&mut self.inner).poll_write(cx, buf) {
            Poll::Ready(result) => {
                self.write_sleep = None;
                Poll::Ready(result)
            }
            Poll::Pending => match self.poll_write_deadline(cx) {
                Poll::Ready(result) => Poll::Ready(result.map(|()| 0)),
                Poll::Pending => Poll::Pending,
            },
        }
    }

    fn poll_flush(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Result<(), io::Error>> {
        Pin::new(&mut self.inner).poll_flush(cx)
    }

    fn poll_shutdown(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
    ) -> Poll<Result<(), io::Error>> {
        Pin::new(&mut self.inner).poll_shutdown(cx)
    }

    fn is_write_vectored(&self) -> bool {
        self.inner.is_write_vectored()
    }

    fn poll_write_vectored(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        bufs: &[io::IoSlice<'_>],
    ) -> Poll<Result<usize, io::Error>> {
        if self.write_sleep.is_some() {
            if let Poll::Ready(result) = self.poll_write_deadline(cx) {
                return Poll::Ready(result.map(|()| 0));
            }
        }
        match Pin::new(&mut self.inner).poll_write_vectored(cx, bufs) {
            Poll::Ready(result) => {
                self.write_sleep = None;
                Poll::Ready(result)
            }
            Poll::Pending => match self.poll_write_deadline(cx) {
                Poll::Ready(result) => Poll::Ready(result.map(|()| 0)),
                Poll::Pending => Poll::Pending,
            },
        }
    }
}

fn log_http_connection_result(
    result: Option<Result<Result<(), NacelleError>, tokio::task::JoinError>>,
) {
    match result {
        Some(Ok(Ok(()))) | None => {}
        Some(Ok(Err(error))) => {
            tracing::debug!(target: "nacelle", transport = "http", error = %error, "connection finished with error");
        }
        Some(Err(error)) => {
            tracing::warn!(target: "nacelle", transport = "http", error = %error, "connection task failed");
        }
    }
}

async fn drain_http_connection_tasks(
    mut connections: tokio::task::JoinSet<Result<(), NacelleError>>,
    drain_timeout: Duration,
    telemetry: NacelleTelemetry,
) {
    telemetry.shutdown_event(
        NacelleTelemetryEventKind::DrainStarted,
        NacelleTransport::Http,
    );
    let drain = async {
        while let Some(result) = connections.join_next().await {
            log_http_connection_result(Some(result));
        }
    };

    if tokio::time::timeout(drain_timeout, drain).await.is_ok() {
        tracing::info!(target: "nacelle", transport = "http", "connection drain completed");
        telemetry.shutdown_event(
            NacelleTelemetryEventKind::DrainCompleted,
            NacelleTransport::Http,
        );
        return;
    }

    let aborted = connections.len();
    tracing::warn!(target: "nacelle", transport = "http", aborted, "connection drain timed out; aborting active tasks");
    telemetry.shutdown_event(
        NacelleTelemetryEventKind::DrainTimedOut,
        NacelleTransport::Http,
    );
    telemetry.connections_aborted(NacelleTransport::Http, aborted);
    connections.abort_all();
    while let Some(result) = connections.join_next().await {
        log_http_connection_result(Some(result));
    }
}

struct HttpBodyStream {
    body: NacelleBody,
    runtime_state: NacelleRuntimeState,
    response_body_bytes: usize,
}

impl Stream for HttpBodyStream {
    type Item = Result<Frame<Bytes>, NacelleError>;

    fn poll_next(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        let this = self.get_mut();
        match Pin::new(&mut this.body).poll_next(cx) {
            Poll::Ready(Some(Ok(chunk))) => {
                let Some(next) = this.response_body_bytes.checked_add(chunk.len()) else {
                    return Poll::Ready(Some(Err(NacelleError::ResourceLimit(
                        "response_body_bytes",
                    ))));
                };
                if next > this.runtime_state.limits().max_response_body_bytes {
                    return Poll::Ready(Some(Err(NacelleError::ResourceLimit(
                        "response_body_bytes",
                    ))));
                }
                this.response_body_bytes = next;
                Poll::Ready(Some(Ok(Frame::data(chunk))))
            }
            Poll::Ready(Some(Err(error))) => Poll::Ready(Some(Err(error))),
            Poll::Ready(None) => Poll::Ready(None),
            Poll::Pending => Poll::Pending,
        }
    }
}

#[allow(dead_code)]
fn empty_body() -> HttpBody {
    Full::new(Bytes::new())
        .map_err(|never: Infallible| match never {})
        .boxed()
}

#[cfg(test)]
mod tests {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    use crate::handler::handler_fn;
    use crate::request::NacelleRequest;

    use super::*;

    #[tokio::test]
    async fn http_server_streams_request_and_response_body() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("listener should bind");
        let addr = listener.local_addr().expect("listener should have addr");
        let server = HyperServer::new(handler_fn(|mut request: NacelleRequest| async move {
            assert_eq!(
                request.http_meta().expect("http metadata").uri.path(),
                "/echo"
            );
            let (tx, body) = NacelleBody::channel(2);
            while let Some(chunk) = request.body.next_chunk().await {
                tx.send(chunk)
                    .await
                    .expect("response receiver should be open");
            }
            drop(tx);
            Ok(NacelleResponse::http(
                StatusCode::CREATED,
                http::HeaderMap::new(),
                body,
            ))
        }));
        let server_task = tokio::spawn(async move { server.serve_listener(listener).await });

        let mut client = tokio::net::TcpStream::connect(addr)
            .await
            .expect("client should connect");
        client
            .write_all(
                b"POST /echo HTTP/1.1\r\n\
                  Host: localhost\r\n\
                  Content-Length: 11\r\n\
                  Connection: close\r\n\
                  \r\n\
                  hello world",
            )
            .await
            .expect("request should write");

        let mut response = Vec::new();
        client
            .read_to_end(&mut response)
            .await
            .expect("response should read");
        let response = String::from_utf8(response).expect("response should be utf8");

        assert!(response.starts_with("HTTP/1.1 201 Created"));
        assert!(response.contains("hello world"));
        server_task.abort();
    }

    #[tokio::test]
    async fn http_server_stops_accepting_when_shutdown_is_requested() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("listener should bind");
        let server = HyperServer::new(handler_fn(|_request: NacelleRequest| async move {
            Ok(NacelleResponse::http_bytes(StatusCode::OK, "ok"))
        }));
        let (shutdown, token) = crate::lifecycle::NacelleShutdown::pair();
        let server_task =
            tokio::spawn(async move { server.serve_listener_with_shutdown(listener, token).await });

        shutdown.shutdown();

        tokio::time::timeout(std::time::Duration::from_secs(1), server_task)
            .await
            .expect("server should stop promptly")
            .expect("server task should join")
            .expect("server should exit cleanly");
    }

    #[tokio::test]
    async fn http_shutdown_aborts_after_drain_deadline() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("listener should bind");
        let addr = listener.local_addr().expect("listener should have addr");
        let runtime_state = crate::limits::NacelleRuntimeState::default();
        let server = HyperServer::new(handler_fn(|_request: NacelleRequest| async move {
            tokio::time::sleep(std::time::Duration::from_secs(60)).await;
            Ok(NacelleResponse::http_bytes(StatusCode::OK, "ok"))
        }))
        .with_runtime_state(runtime_state.clone());
        let (shutdown, token) = crate::lifecycle::NacelleShutdown::pair();
        let server_task = tokio::spawn(async move {
            server
                .serve_listener_with_shutdown_timeout(
                    listener,
                    token,
                    std::time::Duration::from_millis(10),
                )
                .await
        });

        let _client = tokio::net::TcpStream::connect(addr)
            .await
            .expect("client should connect");
        wait_for_active_connections(&runtime_state, 1).await;

        shutdown.shutdown();

        tokio::time::timeout(std::time::Duration::from_secs(1), server_task)
            .await
            .expect("server should stop before test timeout")
            .expect("server task should join")
            .expect("server should exit cleanly");
        assert_eq!(runtime_state.active_connections(), 0);
    }

    #[tokio::test]
    async fn http_slow_header_client_times_out() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("listener should bind");
        let addr = listener.local_addr().expect("listener should have addr");
        let runtime_state = crate::limits::NacelleRuntimeState::new(
            crate::limits::NacelleLimits::default()
                .with_http_header_read_timeout(std::time::Duration::from_millis(25)),
        );
        let server = HyperServer::new(handler_fn(|_request: NacelleRequest| async move {
            Ok(NacelleResponse::http_bytes(StatusCode::OK, "ok"))
        }))
        .with_runtime_state(runtime_state.clone());
        let server_task = tokio::spawn(async move { server.serve_listener(listener).await });

        let mut client = tokio::net::TcpStream::connect(addr)
            .await
            .expect("client should connect");
        client
            .write_all(b"GET / HTTP/1.1\r\nHost: localhost\r\n")
            .await
            .expect("partial request should write");

        let mut buf = [0_u8; 1];
        let read = tokio::time::timeout(std::time::Duration::from_secs(1), client.read(&mut buf))
            .await
            .expect("connection should close before test timeout");
        assert!(read.is_err() || read.expect("read result should be available") == 0);
        wait_for_active_connections(&runtime_state, 0).await;
        server_task.abort();
    }

    #[tokio::test]
    async fn http_handler_error_becomes_500_response() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("listener should bind");
        let addr = listener.local_addr().expect("listener should have addr");
        let server = HyperServer::new(handler_fn(|_request: NacelleRequest| async move {
            Err(NacelleError::handler(std::io::Error::other("boom")))
        }));
        let server_task = tokio::spawn(async move { server.serve_listener(listener).await });

        let mut client = tokio::net::TcpStream::connect(addr)
            .await
            .expect("client should connect");
        client
            .write_all(
                b"GET / HTTP/1.1\r\n\
                  Host: localhost\r\n\
                  Connection: close\r\n\
                  \r\n",
            )
            .await
            .expect("request should write");

        let mut response = Vec::new();
        client
            .read_to_end(&mut response)
            .await
            .expect("response should read");
        let response = String::from_utf8(response).expect("response should be utf8");

        assert!(response.starts_with("HTTP/1.1 500 Internal Server Error"));
        assert!(response.contains("handler error: boom"));
        server_task.abort();
    }

    async fn wait_for_active_connections(
        runtime_state: &crate::limits::NacelleRuntimeState,
        expected: usize,
    ) {
        for _ in 0..100 {
            if runtime_state.active_connections() == expected {
                return;
            }
            tokio::time::sleep(std::time::Duration::from_millis(5)).await;
        }
        panic!(
            "active connections did not reach {expected}; observed {}",
            runtime_state.active_connections()
        );
    }
}
