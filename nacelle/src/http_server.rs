use std::convert::Infallible;
use std::net::SocketAddr;
use std::pin::Pin;
use std::sync::Arc;
use std::task::{Context, Poll};

use bytes::Bytes;
use futures_core::Stream;
use http_body_util::{BodyExt, Full, StreamBody, combinators::BoxBody};
use hyper::body::{Frame, Incoming};
use hyper::server::conn::http1;
use hyper::service::service_fn;
use hyper::{Request, Response, StatusCode};
use hyper_util::rt::TokioIo;

use crate::error::{BoxError, NacelleError};
use crate::handler::Handler;
use crate::limits::NacelleRuntimeState;
use crate::request::{HttpRequestMeta, NacelleBody, NacelleRequest, NacelleRequestMeta};
use crate::response::{NacelleResponse, NacelleResponseMeta};
use crate::telemetry::{NacelleTelemetry, NacelleTransport};

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

    pub async fn serve_listener(
        self,
        listener: tokio::net::TcpListener,
    ) -> Result<(), NacelleError> {
        let server = Arc::new(self);
        loop {
            let (stream, _) = listener.accept().await?;
            let io = TokioIo::new(stream);
            let server = server.clone();
            let connection_permit = match server.runtime_state.acquire_connection() {
                Ok(permit) => permit,
                Err(error) => {
                    server.telemetry.request_failed(
                        NacelleTransport::Http,
                        None,
                        std::time::Duration::ZERO,
                        &error,
                    );
                    continue;
                }
            };
            server.telemetry.connection_opened(NacelleTransport::Http);
            tokio::spawn(async move {
                let _connection_permit = connection_permit;
                let service = service_fn(move |request| {
                    let server = server.clone();
                    async move { server.handle(request).await }
                });
                let _ = http1::Builder::new().serve_connection(io, service).await;
            });
        }
    }

    async fn handle(&self, request: Request<Incoming>) -> Result<Response<HttpBody>, NacelleError> {
        let request_started = std::time::Instant::now();
        let _request_permit = match self.runtime_state.acquire_request() {
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
            body: incoming_to_body(body, self.runtime_state.clone()),
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

fn incoming_to_body(mut incoming: Incoming, runtime_state: NacelleRuntimeState) -> NacelleBody {
    let (tx, body) = NacelleBody::channel(8);
    tokio::spawn(async move {
        let _streaming_permit = match runtime_state.acquire_streaming_task() {
            Ok(permit) => permit,
            Err(error) => {
                let _ = tx.send(Err(error)).await;
                return;
            }
        };
        let mut body_bytes = 0_usize;
        while let Some(frame) = incoming.frame().await {
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
}
