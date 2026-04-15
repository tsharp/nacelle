use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

use crate::error::NacelleError;
use crate::request::{RequestBody, ResponseWriter};

pub type HandlerFuture = Pin<Box<dyn Future<Output = Result<(), NacelleError>> + Send + 'static>>;

/// Implemented by types that handle a single request/response cycle.
///
/// For the common case of a plain async function or closure, prefer [`handler_fn`] which
/// produces a [`BoxedHandler`] with one fewer vtable dispatch than a manual implementation.
/// Use this trait only when you need to carry state that cannot be expressed as a closure.
pub trait Handler<Svc, Req>: Send + Sync + 'static {
    fn call(
        &self,
        svc: Arc<Svc>,
        req: Req,
        body: RequestBody,
        response: ResponseWriter,
    ) -> HandlerFuture;
}

/// A type-erased, cheaply cloneable handler.
///
/// Internally stores `Arc<dyn Fn(...) -> HandlerFuture + Send + Sync>` so that calling a handler
/// requires only **one** vtable dispatch (into the closure) rather than the two that would be
/// needed with a separate wrapper struct (wrapper vtable → inner closure call).
pub struct BoxedHandler<Svc, Req>(
    Arc<dyn Fn(Arc<Svc>, Req, RequestBody, ResponseWriter) -> HandlerFuture + Send + Sync + 'static>,
);

impl<Svc, Req> BoxedHandler<Svc, Req> {
    #[inline]
    pub(crate) fn call(
        &self,
        svc: Arc<Svc>,
        req: Req,
        body: RequestBody,
        response: ResponseWriter,
    ) -> HandlerFuture {
        (self.0)(svc, req, body, response)
    }
}

impl<Svc, Req> Clone for BoxedHandler<Svc, Req> {
    fn clone(&self) -> Self {
        Self(Arc::clone(&self.0))
    }
}

/// Create a [`BoxedHandler`] from an async function or closure.
pub fn handler_fn<Svc, Req, F, Fut>(f: F) -> BoxedHandler<Svc, Req>
where
    Svc: Send + Sync + 'static,
    Req: Send + 'static,
    F: Fn(Arc<Svc>, Req, RequestBody, ResponseWriter) -> Fut + Send + Sync + 'static,
    Fut: Future<Output = Result<(), NacelleError>> + Send + 'static,
{
    BoxedHandler(Arc::new(move |svc, req, body, response| {
        Box::pin(f(svc, req, body, response)) as HandlerFuture
    }))
}

/// Create a [`BoxedHandler`] from a type that implements the [`Handler`] trait.
pub fn handler_from_trait<Svc, Req, H>(handler: H) -> BoxedHandler<Svc, Req>
where
    Svc: Send + Sync + 'static,
    Req: Send + 'static,
    H: Handler<Svc, Req>,
{
    BoxedHandler(Arc::new(move |svc, req, body, response| {
        handler.call(svc, req, body, response)
    }))
}
