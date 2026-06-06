use std::convert::Infallible;
use std::io;
use std::net::SocketAddr;
use std::pin::Pin;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::task::{Context, Poll};
use std::time::Duration;

use bytes::Bytes;
use futures_core::Stream;
use http::header::{HeaderName, HeaderValue};
use http_body_util::{BodyExt, Full, StreamBody, combinators::BoxBody};
use hyper::body::{Body, Frame, Incoming};
use hyper::server::conn::http1;
use hyper::service::service_fn;
use hyper::{Method, Request, Response, StatusCode};
use hyper_util::rt::{TokioIo, TokioTimer};
use tokio::io::{AsyncRead, AsyncWrite};

use nacelle_core::error::{BoxError, NacelleError};
use nacelle_core::handler::Handler;
use nacelle_core::lifecycle::{NacelleDrainDeadline, NacelleShutdownToken};
use nacelle_core::limits::NacelleRuntimeState;
use nacelle_core::request::{HttpRequestMeta, NacelleBody, NacelleRequest, NacelleRequestMeta};
use nacelle_core::response::{NacelleResponse, NacelleResponseMeta};
use nacelle_core::telemetry::{NacelleTelemetry, NacelleTelemetryEventKind, NacelleTransport};
#[cfg(feature = "tls")]
use nacelle_core::tls::NacelleTlsConfig;

type HttpBody = BoxBody<Bytes, BoxError>;

pub struct HyperServer<H = ()> {
    handler: H,
    telemetry: NacelleTelemetry,
    runtime_state: NacelleRuntimeState,
    http_policy: NacelleHttpPolicy,
    access_log_enabled: bool,
}

#[derive(Debug, Clone, Default)]
pub struct NacelleHttpPolicy {
    allowed_hosts: Option<Vec<String>>,
    allowed_methods: Option<Vec<Method>>,
    max_uri_len: Option<usize>,
    max_header_count: Option<usize>,
    max_header_bytes: Option<usize>,
    security_headers: Vec<(HeaderName, HeaderValue)>,
}

impl NacelleHttpPolicy {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn with_allowed_hosts(
        mut self,
        hosts: impl IntoIterator<Item = impl Into<String>>,
    ) -> Self {
        self.allowed_hosts = Some(
            hosts
                .into_iter()
                .map(|host| host.into().trim_end_matches('.').to_ascii_lowercase())
                .collect(),
        );
        self
    }

    pub fn with_allowed_methods(mut self, methods: impl IntoIterator<Item = Method>) -> Self {
        self.allowed_methods = Some(methods.into_iter().collect());
        self
    }

    pub fn with_max_uri_len(mut self, max: usize) -> Self {
        self.max_uri_len = Some(max);
        self
    }

    pub fn with_max_header_count(mut self, max: usize) -> Self {
        self.max_header_count = Some(max);
        self
    }

    pub fn with_max_header_bytes(mut self, max: usize) -> Self {
        self.max_header_bytes = Some(max);
        self
    }

    pub fn with_security_header(mut self, name: HeaderName, value: HeaderValue) -> Self {
        self.security_headers.push((name, value));
        self
    }

    pub fn with_default_security_headers(self) -> Self {
        self.with_security_header(
            http::header::X_CONTENT_TYPE_OPTIONS,
            HeaderValue::from_static("nosniff"),
        )
        .with_security_header(
            HeaderName::from_static("x-frame-options"),
            HeaderValue::from_static("deny"),
        )
        .with_security_header(
            HeaderName::from_static("referrer-policy"),
            HeaderValue::from_static("no-referrer"),
        )
        .with_security_header(
            HeaderName::from_static("cross-origin-resource-policy"),
            HeaderValue::from_static("same-origin"),
        )
    }

    pub fn with_strict_transport_security(mut self, value: HeaderValue) -> Self {
        self.security_headers
            .push((http::header::STRICT_TRANSPORT_SECURITY, value));
        self
    }
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
            http_policy: self.http_policy.clone(),
            access_log_enabled: self.access_log_enabled,
        }
    }
}

impl<H> HyperServer<H>
where
    H: Handler,
{
    pub fn new(handler: H) -> Self {
        let telemetry = NacelleTelemetry::default();
        let runtime_state = NacelleRuntimeState::default();
        telemetry.register_runtime_state(runtime_state.clone());
        Self {
            handler,
            telemetry,
            runtime_state,
            http_policy: NacelleHttpPolicy::default(),
            access_log_enabled: false,
        }
    }

    pub fn with_telemetry(mut self, telemetry: NacelleTelemetry) -> Self {
        telemetry.register_runtime_state(self.runtime_state.clone());
        self.telemetry = telemetry;
        self
    }

    pub fn with_runtime_state(mut self, runtime_state: NacelleRuntimeState) -> Self {
        self.telemetry.register_runtime_state(runtime_state.clone());
        self.runtime_state = runtime_state;
        self
    }

    pub fn with_http_policy(mut self, http_policy: NacelleHttpPolicy) -> Self {
        self.http_policy = http_policy;
        self
    }

    pub fn with_access_log(mut self, enabled: bool) -> Self {
        self.access_log_enabled = enabled;
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

    #[doc(hidden)]
    pub async fn serve_with_shutdown_deadline(
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
        let (_shutdown, token) = nacelle_core::lifecycle::NacelleShutdown::pair();
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

    #[doc(hidden)]
    pub async fn serve_listener_with_shutdown_deadline(
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
                    let (stream, peer_addr) = accepted?;
                    let server = server.clone();
                    let connection_permit = match server
                        .runtime_state
                        .acquire_connection_for_peer(peer_addr.ip())
                    {
                        Ok(permit) => permit,
                        Err(error) => {
                            server
                                .telemetry
                                .connection_rejected(NacelleTransport::Http, connection_rejection_reason(&error));
                            continue;
                        }
                    };
                    server.telemetry.connection_opened(NacelleTransport::Http);
                    connections.spawn(run_http_connection(server, stream, connection_permit));
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

    #[cfg(feature = "tls")]
    pub async fn serve_tls(
        self,
        addr: SocketAddr,
        tls_config: NacelleTlsConfig,
    ) -> Result<(), NacelleError> {
        let listener = tokio::net::TcpListener::bind(addr).await?;
        self.serve_tls_listener(listener, tls_config).await
    }

    #[cfg(feature = "tls")]
    pub async fn serve_tls_with_shutdown(
        self,
        addr: SocketAddr,
        tls_config: NacelleTlsConfig,
        shutdown: NacelleShutdownToken,
    ) -> Result<(), NacelleError> {
        self.serve_tls_with_shutdown_timeout(addr, tls_config, shutdown, Duration::from_secs(30))
            .await
    }

    #[cfg(feature = "tls")]
    pub async fn serve_tls_with_shutdown_timeout(
        self,
        addr: SocketAddr,
        tls_config: NacelleTlsConfig,
        shutdown: NacelleShutdownToken,
        drain_timeout: Duration,
    ) -> Result<(), NacelleError> {
        let listener = tokio::net::TcpListener::bind(addr).await?;
        self.serve_tls_listener_with_shutdown_deadline(
            listener,
            tls_config,
            shutdown,
            NacelleDrainDeadline::new(drain_timeout),
        )
        .await
    }

    #[cfg(feature = "tls")]
    pub async fn serve_tls_listener(
        self,
        listener: tokio::net::TcpListener,
        tls_config: NacelleTlsConfig,
    ) -> Result<(), NacelleError> {
        let (_shutdown, token) = nacelle_core::lifecycle::NacelleShutdown::pair();
        self.serve_tls_listener_with_shutdown(listener, tls_config, token)
            .await
    }

    #[cfg(feature = "tls")]
    pub async fn serve_tls_listener_with_shutdown(
        self,
        listener: tokio::net::TcpListener,
        tls_config: NacelleTlsConfig,
        shutdown: NacelleShutdownToken,
    ) -> Result<(), NacelleError> {
        self.serve_tls_listener_with_shutdown_deadline(
            listener,
            tls_config,
            shutdown,
            NacelleDrainDeadline::default(),
        )
        .await
    }

    #[cfg(feature = "tls")]
    #[doc(hidden)]
    pub async fn serve_tls_listener_with_shutdown_deadline(
        self,
        listener: tokio::net::TcpListener,
        tls_config: NacelleTlsConfig,
        mut shutdown: NacelleShutdownToken,
        drain_deadline: NacelleDrainDeadline,
    ) -> Result<(), NacelleError> {
        let server = Arc::new(self);
        let acceptor = tokio_rustls::TlsAcceptor::from(tls_config.server_config());
        let handshake_timeout = tls_config.handshake_timeout();
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
                    let (stream, peer_addr) = accepted?;
                    let server = server.clone();
                    let acceptor = acceptor.clone();
                    let connection_permit = match server
                        .runtime_state
                        .acquire_connection_for_peer(peer_addr.ip())
                    {
                        Ok(permit) => permit,
                        Err(error) => {
                            server
                                .telemetry
                                .connection_rejected(NacelleTransport::Http, connection_rejection_reason(&error));
                            continue;
                        }
                    };
                    server.telemetry.connection_opened(NacelleTransport::Http);
                    connections.spawn(async move {
                        let tls_stream = match tokio::time::timeout(
                            handshake_timeout,
                            acceptor.accept(stream),
                        )
                        .await
                        {
                            Ok(Ok(stream)) => stream,
                            Ok(Err(error)) => return Err(NacelleError::protocol(error)),
                            Err(_) => {
                                server
                                    .telemetry
                                    .timeout(NacelleTransport::Http, "tls_handshake");
                                return Err(NacelleError::Timeout("tls_handshake"));
                            }
                        };
                        run_http_connection(server, tls_stream, connection_permit).await
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
        let method = request.method().clone();
        let uri = request.uri().clone();
        if let Some(rejection) = validate_http_policy(&self.http_policy, &request) {
            self.telemetry
                .request_rejected(NacelleTransport::Http, rejection.reason);
            self.access_log(
                &method,
                &uri,
                rejection.status,
                0,
                request_started.elapsed(),
                Some(rejection.reason),
            );
            return response_to_http(
                NacelleResponse::http_bytes(rejection.status, rejection.reason),
                self.runtime_state.clone(),
                self.telemetry.clone(),
                &self.http_policy,
            );
        }
        let _request_permit = match self.runtime_state.acquire_request_tracked() {
            Ok(permit) => permit,
            Err(error) => {
                self.telemetry.request_failed(
                    NacelleTransport::Http,
                    None,
                    request_started.elapsed(),
                    &error,
                );
                self.access_log(
                    &method,
                    &uri,
                    StatusCode::SERVICE_UNAVAILABLE,
                    0,
                    request_started.elapsed(),
                    Some("in_flight_requests"),
                );
                return response_to_http(
                    NacelleResponse::http_bytes(StatusCode::SERVICE_UNAVAILABLE, error.to_string()),
                    self.runtime_state.clone(),
                    self.telemetry.clone(),
                    &self.http_policy,
                );
            }
        };
        let (parts, body) = request.into_parts();
        let request_body_bytes = Arc::new(AtomicUsize::new(0));
        let body_len_hint = body
            .size_hint()
            .upper()
            .and_then(|bytes| usize::try_from(bytes).ok());
        let request = NacelleRequest {
            meta: NacelleRequestMeta::Http(HttpRequestMeta {
                method: parts.method,
                uri: parts.uri,
                headers: parts.headers,
            }),
            body: incoming_to_body(
                body,
                body_len_hint,
                request_body_bytes.clone(),
                self.runtime_state.clone(),
                self.telemetry.clone(),
            ),
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
                let request_bytes = request_body_bytes.load(Ordering::Relaxed);
                let response = response_to_http(
                    response,
                    self.runtime_state.clone(),
                    self.telemetry.clone(),
                    &self.http_policy,
                );
                if let Ok(response) = &response {
                    self.access_log(
                        &method,
                        &uri,
                        response.status(),
                        request_bytes,
                        request_started.elapsed(),
                        None,
                    );
                }
                self.telemetry.request_completed(
                    NacelleTransport::Http,
                    None,
                    request_bytes,
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
                let request_bytes = request_body_bytes.load(Ordering::Relaxed);
                let response = response_to_http(
                    NacelleResponse::http_bytes(
                        StatusCode::INTERNAL_SERVER_ERROR,
                        error.to_string(),
                    ),
                    self.runtime_state.clone(),
                    self.telemetry.clone(),
                    &self.http_policy,
                );
                if let Ok(response) = &response {
                    self.access_log(
                        &method,
                        &uri,
                        response.status(),
                        request_bytes,
                        request_started.elapsed(),
                        Some("handler"),
                    );
                }
                self.telemetry.request_completed(
                    NacelleTransport::Http,
                    None,
                    request_bytes,
                    0,
                    request_started.elapsed(),
                );
                response
            }
        }
    }

    fn access_log(
        &self,
        method: &Method,
        uri: &http::Uri,
        status: StatusCode,
        request_bytes: usize,
        elapsed: Duration,
        reason: Option<&'static str>,
    ) {
        if !self.access_log_enabled {
            return;
        }
        tracing::info!(
            target: "nacelle::access",
            transport = "http",
            method = %method,
            uri = %uri,
            status = status.as_u16(),
            request_bytes,
            elapsed_us = elapsed.as_micros() as u64,
            reason,
            "http access"
        );
    }
}

fn incoming_to_body(
    mut incoming: Incoming,
    body_len_hint: Option<usize>,
    request_body_bytes: Arc<AtomicUsize>,
    runtime_state: NacelleRuntimeState,
    telemetry: NacelleTelemetry,
) -> NacelleBody {
    let (tx, body) = NacelleBody::channel(8);
    tokio::spawn(async move {
        if let Some(body_len_hint) = body_len_hint
            && body_len_hint > runtime_state.limits().max_request_body_bytes
        {
            let _ = tx
                .send(Err(NacelleError::ResourceLimit("request_body_bytes")))
                .await;
            return;
        }
        let _body_reservation = match body_len_hint {
            Some(bytes) => match runtime_state.reserve_memory(bytes) {
                Ok(reservation) => Some(reservation),
                Err(error) => {
                    let _ = tx.send(Err(error)).await;
                    return;
                }
            },
            None => None,
        };
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
                        request_body_bytes.fetch_add(data.len(), Ordering::Relaxed);
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
    telemetry: NacelleTelemetry,
    policy: &NacelleHttpPolicy,
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
    apply_security_headers(builder_headers, policy);
    builder
        .body(nacelle_body_to_http(
            response.body,
            runtime_state,
            telemetry,
        ))
        .map_err(NacelleError::protocol)
}

fn apply_security_headers(headers: &mut http::HeaderMap, policy: &NacelleHttpPolicy) {
    for (name, value) in &policy.security_headers {
        if !headers.contains_key(name) {
            headers.insert(name.clone(), value.clone());
        }
    }
}

fn nacelle_body_to_http(
    body: NacelleBody,
    runtime_state: NacelleRuntimeState,
    telemetry: NacelleTelemetry,
) -> HttpBody {
    StreamBody::new(HttpBodyStream {
        body,
        runtime_state,
        telemetry,
        response_body_bytes: 0,
    })
    .map_err(|error| -> BoxError { Box::new(error) })
    .boxed()
}

#[derive(Debug, Clone, Copy)]
struct HttpRejection {
    status: StatusCode,
    reason: &'static str,
}

fn validate_http_policy(
    policy: &NacelleHttpPolicy,
    request: &Request<Incoming>,
) -> Option<HttpRejection> {
    if let Some(max_uri_len) = policy.max_uri_len
        && request.uri().to_string().len() > max_uri_len
    {
        return Some(HttpRejection {
            status: StatusCode::URI_TOO_LONG,
            reason: "uri_too_long",
        });
    }

    if let Some(methods) = &policy.allowed_methods
        && !methods.iter().any(|method| method == request.method())
    {
        return Some(HttpRejection {
            status: StatusCode::METHOD_NOT_ALLOWED,
            reason: "method_not_allowed",
        });
    }

    if let Some(max_header_count) = policy.max_header_count
        && request.headers().len() > max_header_count
    {
        return Some(HttpRejection {
            status: StatusCode::REQUEST_HEADER_FIELDS_TOO_LARGE,
            reason: "header_count",
        });
    }

    if let Some(max_header_bytes) = policy.max_header_bytes {
        let header_bytes = request
            .headers()
            .iter()
            .try_fold(0_usize, |total, (name, value)| {
                total
                    .checked_add(name.as_str().len())?
                    .checked_add(value.as_bytes().len())
            });
        if header_bytes.is_none_or(|bytes| bytes > max_header_bytes) {
            return Some(HttpRejection {
                status: StatusCode::REQUEST_HEADER_FIELDS_TOO_LARGE,
                reason: "header_bytes",
            });
        }
    }

    if let Some(hosts) = &policy.allowed_hosts
        && !host_allowed(hosts, request)
    {
        return Some(HttpRejection {
            status: StatusCode::MISDIRECTED_REQUEST,
            reason: "host",
        });
    }

    None
}

fn host_allowed(allowed_hosts: &[String], request: &Request<Incoming>) -> bool {
    let Some(host) = request
        .headers()
        .get(http::header::HOST)
        .and_then(|host| host.to_str().ok())
    else {
        return false;
    };
    let host = host.trim_end_matches('.').to_ascii_lowercase();
    let host_without_port = host
        .split_once(':')
        .map(|(host, _port)| host)
        .unwrap_or(host.as_str());
    allowed_hosts
        .iter()
        .any(|allowed| allowed == &host || allowed == host_without_port)
}

fn connection_rejection_reason(error: &NacelleError) -> &'static str {
    match error {
        NacelleError::ResourceLimit(reason) => reason,
        _ => "connections",
    }
}

async fn run_http_connection<H, I>(
    server: Arc<HyperServer<H>>,
    stream: I,
    _connection_permit: nacelle_core::limits::TrackedPermit,
) -> Result<(), NacelleError>
where
    H: Handler,
    I: AsyncRead + AsyncWrite + Unpin + Send + 'static,
{
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
        if self.write_sleep.is_some()
            && let Poll::Ready(result) = self.poll_write_deadline(cx)
        {
            return Poll::Ready(result.map(|()| 0));
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
        if self.write_sleep.is_some()
            && let Poll::Ready(result) = self.poll_write_deadline(cx)
        {
            return Poll::Ready(result.map(|()| 0));
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
    telemetry: NacelleTelemetry,
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

impl Drop for HttpBodyStream {
    fn drop(&mut self) {
        self.telemetry
            .response_body_bytes(NacelleTransport::Http, self.response_body_bytes);
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

    use nacelle_core::handler::handler_fn;
    use nacelle_core::request::NacelleRequest;

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
    async fn http_response_body_bytes_are_recorded() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("listener should bind");
        let addr = listener.local_addr().expect("listener should have addr");
        let sink = Arc::new(nacelle_core::NacelleInMemoryTelemetrySink::new());
        let telemetry = NacelleTelemetry::new().with_sink(sink.clone());
        let server = HyperServer::new(handler_fn(|_request: NacelleRequest| async move {
            Ok(NacelleResponse::http_bytes(StatusCode::OK, "hello bytes"))
        }))
        .with_telemetry(telemetry);
        let server_task = tokio::spawn(async move { server.serve_listener(listener).await });

        let mut client = tokio::net::TcpStream::connect(addr)
            .await
            .expect("client should connect");
        client
            .write_all(b"GET / HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n")
            .await
            .expect("request should write");
        let mut response = Vec::new();
        client
            .read_to_end(&mut response)
            .await
            .expect("response should read");

        assert!(sink.events().iter().any(|event| {
            event.kind == nacelle_core::NacelleTelemetryEventKind::ResponseBodyBytes
                && event.transport == Some(NacelleTransport::Http)
                && event.count == "hello bytes".len() as u64
        }));
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
        let (shutdown, token) = nacelle_core::lifecycle::NacelleShutdown::pair();
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
        let runtime_state = nacelle_core::limits::NacelleRuntimeState::default();
        let server = HyperServer::new(handler_fn(|_request: NacelleRequest| async move {
            tokio::time::sleep(std::time::Duration::from_secs(60)).await;
            Ok(NacelleResponse::http_bytes(StatusCode::OK, "ok"))
        }))
        .with_runtime_state(runtime_state.clone());
        let (shutdown, token) = nacelle_core::lifecycle::NacelleShutdown::pair();
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
        let runtime_state = nacelle_core::limits::NacelleRuntimeState::new(
            nacelle_core::limits::NacelleLimits::default()
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
    async fn http_content_length_body_reserves_memory_until_consumed() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("listener should bind");
        let addr = listener.local_addr().expect("listener should have addr");
        let runtime_state = nacelle_core::limits::NacelleRuntimeState::new(
            nacelle_core::limits::NacelleLimits::default().with_max_memory_bytes(1024 * 1024),
        );
        let observed_memory = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let observed = observed_memory.clone();
        let state_for_handler = runtime_state.clone();
        let server = HyperServer::new(handler_fn(move |mut request: NacelleRequest| {
            let observed = observed.clone();
            let state_for_handler = state_for_handler.clone();
            async move {
                for _ in 0..100 {
                    let memory = state_for_handler.memory_used_bytes();
                    if memory >= 11 {
                        observed.store(memory, std::sync::atomic::Ordering::SeqCst);
                        break;
                    }
                    tokio::time::sleep(std::time::Duration::from_millis(2)).await;
                }
                while let Some(chunk) = request.body.next_chunk().await {
                    let _ = chunk?;
                }
                Ok(NacelleResponse::http_bytes(StatusCode::OK, "ok"))
            }
        }))
        .with_runtime_state(runtime_state.clone());
        let server_task = tokio::spawn(async move { server.serve_listener(listener).await });

        let mut client = tokio::net::TcpStream::connect(addr)
            .await
            .expect("client should connect");
        client
            .write_all(
                b"POST / HTTP/1.1\r\n\
                  Host: localhost\r\n\
                  Content-Length: 11\r\n\
                  Connection: close\r\n\
                  \r\n",
            )
            .await
            .expect("headers should write");
        tokio::time::sleep(std::time::Duration::from_millis(25)).await;
        client
            .write_all(b"hello world")
            .await
            .expect("body should write");

        let mut response = Vec::new();
        client
            .read_to_end(&mut response)
            .await
            .expect("response should read");

        assert!(
            observed_memory.load(std::sync::atomic::Ordering::SeqCst) >= 11,
            "handler should observe reserved body memory"
        );
        wait_for_memory(&runtime_state, 0).await;
        server_task.abort();
    }

    #[tokio::test]
    async fn http_trickle_request_body_times_out() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("listener should bind");
        let addr = listener.local_addr().expect("listener should have addr");
        let runtime_state = nacelle_core::limits::NacelleRuntimeState::new(
            nacelle_core::limits::NacelleLimits::default()
                .with_http_request_body_read_timeout(std::time::Duration::from_millis(20)),
        );
        let server = HyperServer::new(handler_fn(|mut request: NacelleRequest| async move {
            while let Some(chunk) = request.body.next_chunk().await {
                let _ = chunk?;
            }
            Ok(NacelleResponse::http_bytes(StatusCode::OK, "ok"))
        }))
        .with_runtime_state(runtime_state);
        let server_task = tokio::spawn(async move { server.serve_listener(listener).await });

        let mut client = tokio::net::TcpStream::connect(addr)
            .await
            .expect("client should connect");
        client
            .write_all(
                b"POST / HTTP/1.1\r\n\
                  Host: localhost\r\n\
                  Content-Length: 11\r\n\
                  Connection: close\r\n\
                  \r\n",
            )
            .await
            .expect("headers should write");

        let mut response = Vec::new();
        tokio::time::timeout(
            std::time::Duration::from_secs(1),
            client.read_to_end(&mut response),
        )
        .await
        .expect("server should respond before test timeout")
        .expect("response should read");
        let response = String::from_utf8(response).expect("response should be utf8");
        assert!(response.starts_with("HTTP/1.1 500 Internal Server Error"));
        assert!(response.contains("http_body_read"));
        server_task.abort();
    }

    #[tokio::test]
    async fn http_slow_response_reader_times_out_or_drains() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("listener should bind");
        let addr = listener.local_addr().expect("listener should have addr");
        let runtime_state = nacelle_core::limits::NacelleRuntimeState::new(
            nacelle_core::limits::NacelleLimits::default()
                .with_http_response_write_timeout(std::time::Duration::from_millis(20)),
        );
        let server = HyperServer::new(handler_fn(|_request: NacelleRequest| async move {
            let (tx, body) = NacelleBody::channel(1);
            tokio::spawn(async move {
                let chunk = Bytes::from(vec![0x5A; 16 * 1024]);
                for _ in 0..512 {
                    if tx.send(Ok(chunk.clone())).await.is_err() {
                        break;
                    }
                }
            });
            Ok(NacelleResponse::http(
                StatusCode::OK,
                http::HeaderMap::new(),
                body,
            ))
        }))
        .with_runtime_state(runtime_state.clone());
        let server_task = tokio::spawn(async move { server.serve_listener(listener).await });

        let mut client = tokio::net::TcpStream::connect(addr)
            .await
            .expect("client should connect");
        client
            .write_all(b"GET / HTTP/1.1\r\nHost: localhost\r\n\r\n")
            .await
            .expect("request should write");
        let mut headers = [0_u8; 128];
        let _ = client
            .read(&mut headers)
            .await
            .expect("response headers should start");

        wait_for_active_connections(&runtime_state, 0).await;
        drop(client);
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

    #[cfg(feature = "tls-self-signed")]
    #[tokio::test]
    async fn http_tls_self_signed_server_accepts_request() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("listener should bind");
        let addr = listener.local_addr().expect("listener should have addr");
        let generated = nacelle_core::tls::NacelleTlsConfig::self_signed(["localhost"])
            .expect("self-signed tls");
        let certificate =
            nacelle_core::tls::parse_pem_certificates(generated.certificate_pem.as_bytes())
                .expect("certificate should parse")
                .remove(0);
        let mut roots = rustls::RootCertStore::empty();
        roots.add(certificate).expect("root cert should add");
        let client_config = rustls::ClientConfig::builder()
            .with_root_certificates(roots)
            .with_no_client_auth();
        let connector = tokio_rustls::TlsConnector::from(std::sync::Arc::new(client_config));
        let server = HyperServer::new(handler_fn(|_request: NacelleRequest| async move {
            Ok(NacelleResponse::http_bytes(StatusCode::OK, "tls ok"))
        }));
        let server_task = tokio::spawn(async move {
            server
                .serve_tls_listener(listener, generated.tls_config)
                .await
        });

        let stream = tokio::net::TcpStream::connect(addr)
            .await
            .expect("client should connect");
        let server_name =
            rustls::pki_types::ServerName::try_from("localhost").expect("valid server name");
        let mut client = connector
            .connect(server_name, stream)
            .await
            .expect("tls should connect");
        client
            .write_all(b"GET / HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n")
            .await
            .expect("request should write");
        let mut response = Vec::new();
        client
            .read_to_end(&mut response)
            .await
            .expect("response should read");
        let response = String::from_utf8(response).expect("response should be utf8");

        assert!(response.starts_with("HTTP/1.1 200 OK"));
        assert!(response.contains("tls ok"));
        server_task.abort();
    }

    #[tokio::test]
    async fn http_policy_rejects_disallowed_host_before_handler() {
        let called = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let called_for_handler = called.clone();
        let (response, events) = http_policy_response(
            NacelleHttpPolicy::new().with_allowed_hosts(["example.com"]),
            handler_fn(move |_request: NacelleRequest| {
                let called_for_handler = called_for_handler.clone();
                async move {
                    called_for_handler.store(true, std::sync::atomic::Ordering::SeqCst);
                    Ok(NacelleResponse::http_bytes(StatusCode::OK, "handler"))
                }
            }),
            b"GET / HTTP/1.1\r\nHost: wrong.example\r\nConnection: close\r\n\r\n",
        )
        .await;

        assert!(response.starts_with("HTTP/1.1 421 Misdirected Request"));
        assert!(!called.load(std::sync::atomic::Ordering::SeqCst));
        assert!(events.iter().any(|event| {
            event.kind == nacelle_core::NacelleTelemetryEventKind::RequestRejected
                && event.reason == Some("host")
        }));
    }

    #[tokio::test]
    async fn http_policy_rejects_disallowed_method_before_handler() {
        let (response, events) = http_policy_response(
            NacelleHttpPolicy::new().with_allowed_methods([Method::GET]),
            handler_fn(|_request: NacelleRequest| async move {
                Ok(NacelleResponse::http_bytes(StatusCode::OK, "handler"))
            }),
            b"POST / HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n",
        )
        .await;

        assert!(response.starts_with("HTTP/1.1 405 Method Not Allowed"));
        assert!(events.iter().any(|event| {
            event.kind == nacelle_core::NacelleTelemetryEventKind::RequestRejected
                && event.reason == Some("method_not_allowed")
        }));
    }

    #[tokio::test]
    async fn http_policy_rejects_long_uri_before_handler() {
        let (response, events) = http_policy_response(
            NacelleHttpPolicy::new().with_max_uri_len(4),
            handler_fn(|_request: NacelleRequest| async move {
                Ok(NacelleResponse::http_bytes(StatusCode::OK, "handler"))
            }),
            b"GET /too-long HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n",
        )
        .await;

        assert!(response.starts_with("HTTP/1.1 414 URI Too Long"));
        assert!(events.iter().any(|event| {
            event.kind == nacelle_core::NacelleTelemetryEventKind::RequestRejected
                && event.reason == Some("uri_too_long")
        }));
    }

    #[tokio::test]
    async fn http_policy_rejects_header_count_and_bytes_before_handler() {
        let (response, events) = http_policy_response(
            NacelleHttpPolicy::new()
                .with_max_header_count(1)
                .with_max_header_bytes(16),
            handler_fn(|_request: NacelleRequest| async move {
                Ok(NacelleResponse::http_bytes(StatusCode::OK, "handler"))
            }),
            b"GET / HTTP/1.1\r\nHost: localhost\r\nX-Long: abcdefghijklmnop\r\nConnection: close\r\n\r\n",
        )
        .await;

        assert!(response.starts_with("HTTP/1.1 431 Request Header Fields Too Large"));
        assert!(events.iter().any(|event| {
            event.kind == nacelle_core::NacelleTelemetryEventKind::RequestRejected
                && matches!(event.reason, Some("header_count" | "header_bytes"))
        }));
    }

    #[tokio::test]
    async fn http_policy_adds_security_headers_to_normal_and_rejected_responses() {
        let policy = NacelleHttpPolicy::new()
            .with_default_security_headers()
            .with_allowed_hosts(["example.com"]);

        let (normal_response, _events) = http_policy_response(
            policy.clone(),
            handler_fn(|_request: NacelleRequest| async move {
                Ok(NacelleResponse::http_bytes(StatusCode::OK, "handler"))
            }),
            b"GET / HTTP/1.1\r\nHost: example.com\r\nConnection: close\r\n\r\n",
        )
        .await;
        assert!(normal_response.starts_with("HTTP/1.1 200 OK"));
        assert!(normal_response.contains("x-content-type-options: nosniff"));
        assert!(normal_response.contains("x-frame-options: deny"));

        let (rejected_response, _events) = http_policy_response(
            policy,
            handler_fn(|_request: NacelleRequest| async move {
                Ok(NacelleResponse::http_bytes(StatusCode::OK, "handler"))
            }),
            b"GET / HTTP/1.1\r\nHost: wrong.example\r\nConnection: close\r\n\r\n",
        )
        .await;
        assert!(rejected_response.starts_with("HTTP/1.1 421 Misdirected Request"));
        assert!(rejected_response.contains("x-content-type-options: nosniff"));
        assert!(rejected_response.contains("x-frame-options: deny"));
    }

    #[tokio::test]
    async fn http_per_peer_connection_limit_rejects_second_connection() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("listener should bind");
        let addr = listener.local_addr().expect("listener should have addr");
        let runtime_state = nacelle_core::limits::NacelleRuntimeState::new(
            nacelle_core::limits::NacelleLimits::default().with_max_connections_per_peer(1),
        );
        let sink = Arc::new(nacelle_core::NacelleInMemoryTelemetrySink::new());
        let telemetry = NacelleTelemetry::new().with_sink(sink.clone());
        let server = HyperServer::new(handler_fn(|_request: NacelleRequest| async move {
            Ok(NacelleResponse::http_bytes(StatusCode::OK, "ok"))
        }))
        .with_runtime_state(runtime_state.clone())
        .with_telemetry(telemetry);
        let server_task = tokio::spawn(async move { server.serve_listener(listener).await });

        let held = tokio::net::TcpStream::connect(addr)
            .await
            .expect("held client should connect");
        wait_for_active_connections(&runtime_state, 1).await;

        let mut rejected = tokio::net::TcpStream::connect(addr)
            .await
            .expect("second client should connect before rejection");
        let mut buf = [0_u8; 1];
        let _ = tokio::time::timeout(std::time::Duration::from_secs(1), rejected.read(&mut buf))
            .await
            .expect("rejected connection should close promptly");

        assert_eq!(runtime_state.active_connections(), 1);
        assert!(sink.events().iter().any(|event| {
            event.kind == nacelle_core::NacelleTelemetryEventKind::ConnectionRejected
                && event.reason == Some("peer_connections")
        }));

        drop(rejected);
        drop(held);
        wait_for_active_connections(&runtime_state, 0).await;
        server_task.abort();
    }

    async fn wait_for_active_connections(
        runtime_state: &nacelle_core::limits::NacelleRuntimeState,
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

    async fn wait_for_memory(
        runtime_state: &nacelle_core::limits::NacelleRuntimeState,
        expected: usize,
    ) {
        for _ in 0..100 {
            if runtime_state.memory_used_bytes() == expected {
                return;
            }
            tokio::time::sleep(std::time::Duration::from_millis(5)).await;
        }
        panic!(
            "memory did not reach {expected}; observed {}",
            runtime_state.memory_used_bytes()
        );
    }

    async fn http_policy_response<H>(
        policy: NacelleHttpPolicy,
        handler: H,
        request: &[u8],
    ) -> (String, Vec<nacelle_core::NacelleTelemetryEvent>)
    where
        H: Handler,
    {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("listener should bind");
        let addr = listener.local_addr().expect("listener should have addr");
        let sink = Arc::new(nacelle_core::NacelleInMemoryTelemetrySink::new());
        let telemetry = NacelleTelemetry::new().with_sink(sink.clone());
        let server = HyperServer::new(handler)
            .with_telemetry(telemetry)
            .with_http_policy(policy);
        let server_task = tokio::spawn(async move { server.serve_listener(listener).await });

        let mut client = tokio::net::TcpStream::connect(addr)
            .await
            .expect("client should connect");
        client
            .write_all(request)
            .await
            .expect("request should write");
        let mut response = Vec::new();
        client
            .read_to_end(&mut response)
            .await
            .expect("response should read");
        server_task.abort();

        (
            String::from_utf8(response).expect("response should be utf8"),
            sink.events(),
        )
    }
}
