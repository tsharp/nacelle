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
use crate::request::{HttpRequestMeta, NacelleBody, NacelleRequest, NacelleRequestMeta};
use crate::response::{NacelleResponse, NacelleResponseMeta};

type HttpBody = BoxBody<Bytes, BoxError>;

pub struct HyperServer<H = ()> {
    handler: H,
}

impl<H> Clone for HyperServer<H>
where
    H: Clone,
{
    fn clone(&self) -> Self {
        Self {
            handler: self.handler.clone(),
        }
    }
}

impl<H> HyperServer<H>
where
    H: Handler,
{
    pub fn new(handler: H) -> Self {
        Self { handler }
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
            tokio::spawn(async move {
                let service = service_fn(move |request| {
                    let server = server.clone();
                    async move { server.handle(request).await }
                });
                let _ = http1::Builder::new().serve_connection(io, service).await;
            });
        }
    }

    async fn handle(&self, request: Request<Incoming>) -> Result<Response<HttpBody>, NacelleError> {
        let (parts, body) = request.into_parts();
        let request = NacelleRequest {
            meta: NacelleRequestMeta::Http(HttpRequestMeta {
                method: parts.method,
                uri: parts.uri,
                headers: parts.headers,
            }),
            body: incoming_to_body(body),
        };

        match self.handler.call(request).await {
            Ok(response) => response_to_http(response),
            Err(error) => response_to_http(NacelleResponse::http_bytes(
                StatusCode::INTERNAL_SERVER_ERROR,
                error.to_string(),
            )),
        }
    }
}

fn incoming_to_body(mut incoming: Incoming) -> NacelleBody {
    let (tx, body) = NacelleBody::channel(8);
    tokio::spawn(async move {
        while let Some(frame) = incoming.frame().await {
            match frame {
                Ok(frame) => {
                    if let Some(data) = frame.data_ref()
                        && tx.send(Ok(data.clone())).await.is_err()
                    {
                        break;
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

fn response_to_http(response: NacelleResponse) -> Result<Response<HttpBody>, NacelleError> {
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
        .body(nacelle_body_to_http(response.body))
        .map_err(NacelleError::protocol)
}

fn nacelle_body_to_http(body: NacelleBody) -> HttpBody {
    StreamBody::new(HttpBodyStream { body })
        .map_err(|error| -> BoxError { Box::new(error) })
        .boxed()
}

struct HttpBodyStream {
    body: NacelleBody,
}

impl Stream for HttpBodyStream {
    type Item = Result<Frame<Bytes>, NacelleError>;

    fn poll_next(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        let this = self.get_mut();
        match Pin::new(&mut this.body).poll_next(cx) {
            Poll::Ready(Some(Ok(chunk))) => Poll::Ready(Some(Ok(Frame::data(chunk)))),
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
