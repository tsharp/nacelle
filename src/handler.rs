use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

use crate::error::CascadeError;
use crate::request::{RequestBody, ResponseWriter};

pub type HandlerFuture = Pin<Box<dyn Future<Output = Result<(), CascadeError>> + Send + 'static>>;

pub trait Handler<Svc, Req>: Send + Sync + 'static {
    fn call(
        &self,
        svc: Arc<Svc>,
        req: Req,
        body: RequestBody,
        response: ResponseWriter,
    ) -> HandlerFuture;
}

pub type BoxedHandler<Svc, Req> = Arc<dyn Handler<Svc, Req>>;

struct FnHandler<F> {
    inner: F,
}

impl<Svc, Req, F, Fut> Handler<Svc, Req> for FnHandler<F>
where
    Svc: Send + Sync + 'static,
    Req: Send + 'static,
    F: Fn(Arc<Svc>, Req, RequestBody, ResponseWriter) -> Fut + Send + Sync + 'static,
    Fut: Future<Output = Result<(), CascadeError>> + Send + 'static,
{
    fn call(
        &self,
        svc: Arc<Svc>,
        req: Req,
        body: RequestBody,
        response: ResponseWriter,
    ) -> HandlerFuture {
        Box::pin((self.inner)(svc, req, body, response))
    }
}

pub fn handler_fn<Svc, Req, F, Fut>(handler: F) -> BoxedHandler<Svc, Req>
where
    Svc: Send + Sync + 'static,
    Req: Send + 'static,
    F: Fn(Arc<Svc>, Req, RequestBody, ResponseWriter) -> Fut + Send + Sync + 'static,
    Fut: Future<Output = Result<(), CascadeError>> + Send + 'static,
{
    Arc::new(FnHandler { inner: handler })
}
