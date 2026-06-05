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
use crate::handler::BoxedHandler;
use crate::request::{HttpRequestMeta, NacelleBody, NacelleRequest, NacelleRequestMeta};
use crate::response::{NacelleResponse, NacelleResponseMeta};

type HttpBody = BoxBody<Bytes, BoxError>;

pub struct HyperServer<Svc> {
    service: Arc<Svc>,
    handler: BoxedHandler<Svc>,
}

impl<Svc> Clone for HyperServer<Svc> {
    fn clone(&self) -> Self {
        Self {
            service: self.service.clone(),
            handler: self.handler.clone(),
        }
    }
}

impl<Svc> HyperServer<Svc>
where
    Svc: Send + Sync + 'static,
{
    pub fn new(service: Svc, handler: BoxedHandler<Svc>) -> Self {
        Self {
            service: Arc::new(service),
            handler,
        }
    }

    pub async fn serve(self, addr: SocketAddr) -> Result<(), NacelleError> {
        let listener = tokio::net::TcpListener::bind(addr).await?;
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

        match self.handler.call(self.service.clone(), request).await {
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
