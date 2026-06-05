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
/// For the common case of a plain async function or closure, prefer [`handler_fn`]. It returns a
/// concrete handler type, so the server can monomorphize the handler call and future instead of
/// paying a boxed-future allocation per request.
pub trait Handler<Svc>: Clone + Send + Sync + 'static {
    type Future: Future<Output = Result<NacelleResponse, NacelleError>> + Send + 'static;

    fn call(&self, svc: Arc<Svc>, request: NacelleRequest) -> Self::Future;
}

#[derive(Clone)]
pub struct HandlerFn<F>(F);

impl<Svc, F, Fut> Handler<Svc> for HandlerFn<F>
where
    Svc: Send + Sync + 'static,
    F: Fn(Arc<Svc>, NacelleRequest) -> Fut + Clone + Send + Sync + 'static,
    Fut: Future<Output = Result<NacelleResponse, NacelleError>> + Send + 'static,
{
    type Future = Fut;

    #[inline]
    fn call(&self, svc: Arc<Svc>, request: NacelleRequest) -> Self::Future {
        (self.0)(svc, request)
    }
}

/// A type-erased, cheaply cloneable handler.
///
/// Internally stores `Arc<dyn Fn(...) -> HandlerFuture + Send + Sync>` so that calling a handler
/// requires only one vtable dispatch into the closure. Prefer [`handler_fn`] on latency-sensitive
/// paths.
pub struct BoxedHandler<Svc>(
    Arc<dyn Fn(Arc<Svc>, NacelleRequest) -> HandlerFuture + Send + Sync + 'static>,
);

impl<Svc> Handler<Svc> for BoxedHandler<Svc>
where
    Svc: Send + Sync + 'static,
{
    type Future = HandlerFuture;

    #[inline]
    fn call(&self, svc: Arc<Svc>, request: NacelleRequest) -> Self::Future {
        (self.0)(svc, request)
    }
}

impl<Svc> Clone for BoxedHandler<Svc> {
    fn clone(&self) -> Self {
        Self(Arc::clone(&self.0))
    }
}

/// Create a concrete handler from an async function or closure.
pub fn handler_fn<F>(f: F) -> HandlerFn<F> {
    HandlerFn(f)
}

/// Create a [`BoxedHandler`] from a type that implements the [`Handler`] trait.
pub fn handler_from_trait<Svc, H>(handler: H) -> BoxedHandler<Svc>
where
    Svc: Send + Sync + 'static,
    H: Handler<Svc>,
{
    BoxedHandler(Arc::new(move |svc, request| {
        Box::pin(handler.call(svc, request)) as HandlerFuture
    }))
}
