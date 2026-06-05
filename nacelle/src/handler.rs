use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

use crate::error::NacelleError;
use crate::request::NacelleRequest;
use crate::response::NacelleResponse;

pub type HandlerFuture =
    Pin<Box<dyn Future<Output = Result<NacelleResponse, NacelleError>> + Send + 'static>>;

/// Implemented by types that handle a single request/response cycle.
///
/// For the common case of a plain async function or closure, prefer [`handler_fn`] which
/// produces a [`BoxedHandler`] with one fewer vtable dispatch than a manual implementation.
/// Use this trait only when you need to carry state that cannot be expressed as a closure.
pub trait Handler<Svc>: Send + Sync + 'static {
    fn call(&self, svc: Arc<Svc>, request: NacelleRequest) -> HandlerFuture;
}

/// A type-erased, cheaply cloneable handler.
///
/// Internally stores `Arc<dyn Fn(...) -> HandlerFuture + Send + Sync>` so that calling a handler
/// requires only **one** vtable dispatch (into the closure) rather than the two that would be
/// needed with a separate wrapper struct (wrapper vtable → inner closure call).
pub struct BoxedHandler<Svc>(
    Arc<dyn Fn(Arc<Svc>, NacelleRequest) -> HandlerFuture + Send + Sync + 'static>,
);

impl<Svc> BoxedHandler<Svc> {
    #[inline]
    pub(crate) fn call(&self, svc: Arc<Svc>, request: NacelleRequest) -> HandlerFuture {
        (self.0)(svc, request)
    }
}

impl<Svc> Clone for BoxedHandler<Svc> {
    fn clone(&self) -> Self {
        Self(Arc::clone(&self.0))
    }
}

/// Create a [`BoxedHandler`] from an async function or closure.
pub fn handler_fn<Svc, F, Fut>(f: F) -> BoxedHandler<Svc>
where
    Svc: Send + Sync + 'static,
    F: Fn(Arc<Svc>, NacelleRequest) -> Fut + Send + Sync + 'static,
    Fut: Future<Output = Result<NacelleResponse, NacelleError>> + Send + 'static,
{
    BoxedHandler(Arc::new(move |svc, request| {
        Box::pin(f(svc, request)) as HandlerFuture
    }))
}

/// Create a [`BoxedHandler`] from a type that implements the [`Handler`] trait.
pub fn handler_from_trait<Svc, H>(handler: H) -> BoxedHandler<Svc>
where
    Svc: Send + Sync + 'static,
    H: Handler<Svc>,
{
    BoxedHandler(Arc::new(move |svc, request| handler.call(svc, request)))
}
