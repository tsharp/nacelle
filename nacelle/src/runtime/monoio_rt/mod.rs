//! Monoio runtime backend.
//!
//! monoio is a thread-per-core async runtime backed by io_uring (Linux) or
//! kqueue (macOS/BSD).  Futures are pinned to the thread that spawned them and
//! are never migrated, so `Send` is not required for spawned tasks.
//!
//! This module normalises monoio's `JoinHandle<T>` — whose `Future::Output` is
//! `T` — into `Result<T, JoinError>` so that call-sites can use the same
//! `handle.await??` pattern regardless of which runtime feature is active.
//!
//! I/O is bridged to nacelle's `NacelleRead`/`NacelleWrite` traits by
//! implementing them directly for monoio's owned TCP stream halves.  No
//! external compat crate is required.

use std::future::Future;
use std::pin::Pin;
use std::task::{Context, Poll};

// ── JoinError ─────────────────────────────────────────────────────────────────

/// A task join error for the monoio runtime.
///
/// monoio does not propagate task panics to the joiner, so this type is
/// uninhabited (it can never actually be constructed).  It exists solely for
/// API symmetry with the tokio runtime backend.
#[derive(Debug)]
pub struct JoinError(std::convert::Infallible);

impl std::fmt::Display for JoinError {
    fn fmt(&self, _f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // Infallible — this branch is dead code.
        match self.0 {}
    }
}

impl std::error::Error for JoinError {}

// ── JoinHandle ────────────────────────────────────────────────────────────────

/// A join handle whose output is always `Result<T, JoinError>`, matching the
/// interface of every other runtime backend in this crate.
///
/// Because `JoinError` is uninhabited under monoio, awaiting this handle always
/// produces `Ok(value)`.
pub struct JoinHandle<T>(monoio::task::JoinHandle<T>);

impl<T> JoinHandle<T> {
    /// Pin-projects to the inner monoio `JoinHandle`.
    ///
    /// # Safety
    /// `JoinHandle` is a transparent newtype; the inner field is never moved
    /// after first use as a `Pin`, satisfying the structural-pin contract.
    fn project(self: Pin<&mut Self>) -> Pin<&mut monoio::task::JoinHandle<T>> {
        unsafe { self.map_unchecked_mut(|h| &mut h.0) }
    }
}

impl<T> Future for JoinHandle<T> {
    type Output = Result<T, JoinError>;

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        // monoio JoinHandle resolves to T; we lift into Ok(T) for API symmetry.
        self.project().poll(cx).map(Ok)
    }
}

/// Spawn a `'static` future onto the monoio thread-local executor.
///
/// Unlike the tokio backend, `Send` is **not** required — futures stay on the
/// thread that spawned them.
pub fn spawn<F>(future: F) -> JoinHandle<F::Output>
where
    F: Future + 'static,
    F::Output: 'static,
{
    JoinHandle(monoio::spawn(future))
}

// ── TCP accept loop ───────────────────────────────────────────────────────────

#[cfg(feature = "tcp")]
use std::net::SocketAddr;
#[cfg(feature = "tcp")]
use std::sync::Arc;

#[cfg(feature = "tcp")]
use crate::error::NacelleError;
#[cfg(feature = "tcp")]
use crate::protocol::Protocol;
#[cfg(feature = "tcp")]
use crate::request::RequestMetadata;
#[cfg(feature = "tcp")]
use crate::server::NacelleServer;

// ── NacelleRead / NacelleWrite for monoio owned TCP halves ───────────────────
//
// monoio uses a completion-based I/O model (`AsyncReadRent` / `AsyncWriteRent`)
// where the runtime owns the buffer for the duration of an operation.  We
// bridge that to nacelle's `NacelleRead`/`NacelleWrite` traits by:
//   • read_buf  — allocating a Vec for the in-flight read, then extending the
//                 caller-owned BytesMut once the kernel returns the data.
//   • write_all — copying the caller's slice into an owned Vec so monoio can
//                 hold it during the kernel write.
//   • flush     — delegating to monoio's flush (no-op for TCP in practice).
//
// The extra allocation is network-latency-dominated and negligible in practice.

impl crate::runtime::NacelleRead for monoio::net::tcp::TcpOwnedReadHalf {
    async fn read_buf(&mut self, dst: &mut bytes::BytesMut) -> std::io::Result<usize> {
        let cap = (dst.capacity() - dst.len()).max(4096);
        let buf = vec![0u8; cap];
        let (result, buf) = monoio::io::AsyncReadRent::read(self, buf).await;
        let n = result?;
        dst.extend_from_slice(&buf[..n]);
        Ok(n)
    }
}

impl crate::runtime::NacelleWrite for monoio::net::tcp::TcpOwnedWriteHalf {
    async fn write_all(&mut self, src: &[u8]) -> std::io::Result<()> {
        let buf = src.to_vec();
        let (result, _) = monoio::io::AsyncWriteRentExt::write_all(self, buf).await;
        result.map(|_| ())
    }

    async fn flush(&mut self) -> std::io::Result<()> {
        monoio::io::AsyncWriteRent::flush(self).await
    }
}

/// Listen on `addr` and serve each accepted TCP connection on the current
/// monoio thread-local executor.  Each connection is driven in its own task.
///
/// The accepted `TcpStream` is split into owned read/write halves which are
/// passed directly to `serve_halves` — no compat shim required.
#[cfg(feature = "tcp")]
pub async fn serve_tcp<Svc, Req, P>(
    server: Arc<NacelleServer<Svc, Req, P>>,
    addr: SocketAddr,
) -> Result<(), NacelleError>
where
    Svc: Send + Sync + 'static,
    Req: RequestMetadata + Send + 'static,
    P: Protocol<Req> + Send + Sync + 'static,
{
    use monoio::io::Splitable as _;
    let listener = monoio::net::TcpListener::bind(addr)?;
    loop {
        let (stream, _) = listener.accept().await?;
        let _ = stream.set_nodelay(true);
        let (reader, writer) = stream.into_split();
        let server = server.clone();
        monoio::spawn(async move {
            let _ = server.serve_halves(reader, writer).await;
        });
    }
}
