use std::future::Future;

use crate::error::NacelleError;
use crate::request::NacelleRequest;
use crate::response::NacelleResponse;

/// Implemented by types that handle a single request/response cycle.
///
/// For the common case of a plain async function or closure, prefer [`handler_fn`]. It returns a
/// concrete handler type, so the server can monomorphize the handler call and future instead of
/// paying a boxed-future allocation per request.
pub trait Handler: Clone + Send + Sync + 'static {
    type Future: Future<Output = Result<NacelleResponse, NacelleError>> + Send + 'static;

    fn call(&self, request: NacelleRequest) -> Self::Future;
}

#[derive(Clone)]
pub struct HandlerFn<F>(F);

impl<F, Fut> Handler for HandlerFn<F>
where
    F: Fn(NacelleRequest) -> Fut + Clone + Send + Sync + 'static,
    Fut: Future<Output = Result<NacelleResponse, NacelleError>> + Send + 'static,
{
    type Future = Fut;

    #[inline]
    fn call(&self, request: NacelleRequest) -> Self::Future {
        (self.0)(request)
    }
}

/// Create a concrete handler from an async function or closure.
pub fn handler_fn<F>(f: F) -> HandlerFn<F> {
    HandlerFn(f)
}
