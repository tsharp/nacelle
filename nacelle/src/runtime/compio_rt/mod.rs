//! Compio runtime backend.
//!
//! compio is a thread-per-core async runtime backed by io_uring (Linux),
//! IOCP (Windows), or polling (others).  Futures stay on the thread that
//! spawned them, so `Send` is not required.
//!
//! compio uses a completion-based I/O model: the kernel owns the buffer for
//! the duration of an operation, and the buffer is returned alongside the
//! result.  We bridge this to nacelle's `NacelleRead`/`NacelleWrite` traits
//! by using an intermediate `Vec<u8>` for each I/O operation.

use std::future::Future;
use std::pin::Pin;
use std::task::{Context, Poll};

// ── JoinError ─────────────────────────────────────────────────────────────────

/// A task join error for the compio runtime.  Wraps the erased panic value
/// that compio surfaces when a spawned task panics.
pub struct JoinError(#[allow(dead_code)] Box<dyn std::any::Any + Send + 'static>);

impl std::fmt::Debug for JoinError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("JoinError").finish_non_exhaustive()
    }
}

impl std::fmt::Display for JoinError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "compio task panicked")
    }
}

impl std::error::Error for JoinError {}

// ── JoinHandle ────────────────────────────────────────────────────────────────

/// A join handle whose output is always `Result<T, JoinError>`, matching the
/// interface of every other runtime backend in this crate.
///
/// compio's `spawn` already returns `Task<Result<T, Box<dyn Any + Send>>>`
/// (its own `JoinHandle<T>` alias), so we wrap it to use nacelle's `JoinError`.
pub struct JoinHandle<T>(compio_runtime::JoinHandle<T>);

impl<T> Future for JoinHandle<T> {
    type Output = Result<T, JoinError>;

    fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        Pin::new(&mut self.0).poll(cx).map(|r| r.map_err(JoinError))
    }
}

/// Spawn a `'static` future onto the compio thread-local executor.
///
/// `Send` is **not** required — futures stay on the thread that spawned them.
pub fn spawn<F>(future: F) -> JoinHandle<F::Output>
where
    F: Future + 'static,
    F::Output: 'static,
{
    JoinHandle(compio_runtime::spawn(future))
}

// ── NacelleRead / NacelleWrite for compio owned TCP halves ───────────────────
//
// compio uses a completion-based I/O model where the kernel owns the buffer
// during a read or write.  Buffers must implement `IoBufMut` (read) or `IoBuf`
// (write) from compio-buf — essentially, they must be owned.
//
// We bridge to nacelle's slice-oriented `NacelleRead`/`NacelleWrite` traits by:
//   • read_buf  — allocating a Vec for the in-flight read, then extending the
//                 caller-owned BytesMut with the returned bytes.
//   • write_all — copying the caller's slice into an owned Vec so compio can
//                 hold it during the kernel write.
//   • flush     — delegating to compio_io::AsyncWrite::flush (TCP no-op).

impl crate::runtime::NacelleRead for compio_net::OwnedReadHalf<compio_net::TcpStream> {
    async fn read_buf(&mut self, dst: &mut bytes::BytesMut) -> std::io::Result<usize> {
        let spare = dst.capacity() - dst.len();
        let cap = if spare == 0 { 4096 } else { spare };
        let buf = vec![0u8; cap];
        let result = compio_io::AsyncRead::read(self, buf).await;
        let n = result.0?;
        dst.extend_from_slice(&result.1[..n]);
        Ok(n)
    }
}

impl crate::runtime::NacelleWrite for compio_net::OwnedWriteHalf<compio_net::TcpStream> {
    async fn write_all(&mut self, src: &[u8]) -> std::io::Result<()> {
        let buf = src.to_vec();
        let result = compio_io::AsyncWriteExt::write_all(self, buf).await;
        result.0
    }

    async fn flush(&mut self) -> std::io::Result<()> {
        compio_io::AsyncWrite::flush(self).await
    }
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

/// Listen on `addr` and serve each accepted TCP connection on the current
/// compio thread-local executor.  Each connection is driven in its own task.
///
/// The accepted `TcpStream` is split into owned read/write halves via
/// `into_split` and passed to `serve_halves` — no compat shim required.
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
    let listener = {
        use socket2::{Domain, Protocol, Socket, Type};
        let domain = if addr.is_ipv4() {
            Domain::IPV4
        } else {
            Domain::IPV6
        };
        let sock = Socket::new(domain, Type::STREAM, Some(Protocol::TCP))?;
        sock.set_reuse_address(true)?;
        #[cfg(target_os = "linux")]
        sock.set_reuse_port(true)?;
        sock.set_nonblocking(true)?;
        sock.bind(&addr.into())?;
        sock.listen(128)?;
        compio_net::TcpListener::from_std(sock.into())?
    };
    loop {
        let (stream, _) = listener.accept().await?;
        let _ = stream.set_nodelay(true);
        let (reader, writer) = stream.into_split();
        let server = server.clone();
        compio_runtime::spawn(async move {
            let _ = server.serve_halves(reader, writer).await;
        })
        .detach();
    }
}
