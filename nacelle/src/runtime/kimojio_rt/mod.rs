//! Kimojio runtime backend.
//!
//! kimojio is a thread-per-core async runtime backed by io_uring (Linux),
//! epoll (Linux), or IOCP (Windows).  Futures stay on the thread that spawned
//! them, so `Send` is not required.
//!
//! I/O uses kimojio's `AsyncStreamRead`/`AsyncStreamWrite` traits.  We bridge
//! these to nacelle's `NacelleRead`/`NacelleWrite` traits by wrapping the
//! accepted `OwnedFd` in an `OwnedFdStream` and splitting it into independent
//! read/write halves.

use std::future::Future;
use std::pin::Pin;
use std::task::{Context, Poll};

// ── JoinError ─────────────────────────────────────────────────────────────────

/// A task join error for the kimojio runtime.
///
/// Wraps `kimojio::operations::TaskHandleError`, which covers task panics and
/// explicit cancellation.
pub struct JoinError(kimojio::TaskHandleError);

impl std::fmt::Debug for JoinError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "JoinError({:?})", self.0)
    }
}

impl std::fmt::Display for JoinError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "kimojio task error: {:?}", self.0)
    }
}

impl std::error::Error for JoinError {}

// ── JoinHandle ────────────────────────────────────────────────────────────────

/// A join handle whose output is always `Result<T, JoinError>`, matching the
/// interface of every other runtime backend in this crate.
///
/// kimojio's `TaskHandle<T>` already resolves to `Result<T, TaskHandleError>`,
/// so we just wrap and re-map the error type.
pub struct JoinHandle<T: 'static>(kimojio::operations::TaskHandle<T>);

impl<T: 'static> Future for JoinHandle<T> {
    type Output = Result<T, JoinError>;

    fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        Pin::new(&mut self.0).poll(cx).map(|r| r.map_err(JoinError))
    }
}

/// Spawn a `'static` future onto the kimojio thread-local executor.
///
/// `Send` is **not** required — futures stay on the thread that spawned them.
pub fn spawn<F>(future: F) -> JoinHandle<F::Output>
where
    F: Future + 'static,
    F::Output: 'static,
{
    JoinHandle(kimojio::operations::spawn_task(future))
}

// ── NacelleRead / NacelleWrite for kimojio owned stream halves ───────────────
//
// kimojio uses `AsyncStreamRead::try_read` and `AsyncStreamWrite::write` with
// a mutable slice and an optional deadline.  We bridge to nacelle's
// `NacelleRead`/`NacelleWrite` traits via an intermediate `Vec<u8>` for reads
// and passing the caller's slice directly for writes.

fn kimojio_err(e: kimojio::Errno) -> std::io::Error {
    std::io::Error::from_raw_os_error(e.raw_os_error())
}

impl crate::runtime::NacelleRead for kimojio::OwnedFdStreamRead {
    async fn read_buf(&mut self, dst: &mut bytes::BytesMut) -> std::io::Result<usize> {
        use kimojio::AsyncStreamRead;
        // Use the caller's spare capacity as the ceiling so we never read more
        // bytes than callers such as pump_request_body expect.  4096 is the
        // floor only when dst is completely full (spare == 0).
        let spare = dst.capacity() - dst.len();
        let cap = if spare == 0 { 4096 } else { spare };
        let mut buf = vec![0u8; cap];
        let n = self.try_read(&mut buf, None).await.map_err(kimojio_err)?;
        dst.extend_from_slice(&buf[..n]);
        Ok(n)
    }
}

impl crate::runtime::NacelleWrite for kimojio::OwnedFdStreamWrite {
    async fn write_all(&mut self, src: &[u8]) -> std::io::Result<()> {
        use kimojio::AsyncStreamWrite;
        self.write(src, None).await.map_err(kimojio_err)
    }

    async fn flush(&mut self) -> std::io::Result<()> {
        // TCP doesn't buffer at the kimojio stream layer; no-op.
        Ok(())
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
/// kimojio thread-local executor.  Each connection is driven in its own task.
///
/// The accepted fd is wrapped in an `OwnedFdStream`, split into read/write
/// halves, and handed to `serve_halves` — no compat shim required.
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
    use kimojio::operations::{self, AddressFamily, SocketType, ipproto};
    use kimojio::socket_helpers::update_accept_socket;
    use kimojio::{OwnedFdStream, SplittableStream};

    let family = if addr.is_ipv4() {
        AddressFamily::INET
    } else {
        AddressFamily::INET6
    };

    let server_fd = operations::socket(family, SocketType::STREAM, Some(ipproto::TCP))
        .await
        .map_err(kimojio_err)?;

    #[cfg(target_os = "linux")]
    rustix::net::sockopt::set_socket_reuseport(&server_fd, true)
        .map_err(|e| NacelleError::Io(std::io::Error::from(e)))?;
    rustix::net::sockopt::set_socket_reuseaddr(&server_fd, true)
        .map_err(|e| NacelleError::Io(std::io::Error::from(e)))?;

    operations::bind(&server_fd, &addr).map_err(kimojio_err)?;
    operations::listen(&server_fd, 128).map_err(kimojio_err)?;
    #[cfg(not(windows))]
    {
    // The listening socket must also be non-blocking so accept() returns EAGAIN
    // when no connection is ready, allowing the epoll driver to yield correctly.
    rustix::fs::fcntl_setfl(
        &server_fd,
        rustix::fs::fcntl_getfl(&server_fd).map_err(kimojio_err)? | rustix::fs::OFlags::NONBLOCK,
    )
    .map_err(kimojio_err)?;
    }

    loop {
        let client_fd = operations::accept(&server_fd).await.map_err(kimojio_err)?;
        #[cfg(not(windows))]
        {
        // Accepted sockets don't inherit O_NONBLOCK from the listening socket;
        // kimojio's epoll futures require non-blocking fds or they block the thread.
        rustix::fs::fcntl_setfl(
            &client_fd,
            rustix::fs::fcntl_getfl(&client_fd).map_err(kimojio_err)?
                | rustix::fs::OFlags::NONBLOCK,
        )
        .map_err(kimojio_err)?;
        }
        let _ = update_accept_socket(&client_fd);
        let stream = OwnedFdStream::new(client_fd);
        let (reader, writer) = stream.split().await.map_err(kimojio_err)?;
        let server = server.clone();
        kimojio::operations::spawn_task(async move {
            let _ = server.serve_halves(reader, writer).await;
        });
    }
}
