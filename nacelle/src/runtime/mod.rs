//! Runtime abstraction layer.
//!
//! Provides a uniform `spawn`, `JoinHandle`, and `JoinError` surface — plus the
//! TCP accept loop — that works identically across all supported runtimes.
//! **Exactly one runtime feature** (any feature ending in `-runtime`) may be
//! active at a time; this is enforced by `build.rs` at compile time.
//!
//! Adding a third runtime means adding a new `<name>_rt.rs` submodule that
//! exports the same symbols and threading it in below.

// ── MaybeSend ─────────────────────────────────────────────────────────────────
//
// `MaybeSend` is a marker that means "must be Send under multi-threaded
// runtimes".  Under monoio (thread-per-core), futures and I/O handles never
// cross thread boundaries, so the constraint is dropped to a no-op blanket.
// This lets nacelle accept `!Send` I/O wrappers (such as `monoio-compat`'s
// `TcpStreamCompat`) without a separate API surface.

/// Requires `Send` under multi-threaded runtimes (tokio); no-op under
/// thread-per-core runtimes (monoio).
#[cfg(all(feature = "tokio-runtime", not(feature = "monoio-runtime")))]
pub trait MaybeSend: Send {}

#[cfg(all(feature = "tokio-runtime", not(feature = "monoio-runtime")))]
impl<T: Send> MaybeSend for T {}

#[cfg(feature = "monoio-runtime")]
pub trait MaybeSend {}

#[cfg(feature = "monoio-runtime")]
impl<T> MaybeSend for T {}

// ── NacelleRead / NacelleWrite ────────────────────────────────────────────────
//
// Runtime-agnostic async I/O traits for the readable and writable halves of a
// connection.  Each runtime backend provides implementations for its native
// stream types.  `connection.rs` only sees these traits — no concrete runtime
// types, no compat shims.

/// A readable half of an async I/O stream.
// `async fn` in traits intentionally omits a `+ Send` bound so that the same
// trait works for both tokio (Send futures) and monoio (!Send futures).
// Sendability is inferred by the compiler at each monomorphisation site.
#[allow(async_fn_in_trait)]
pub trait NacelleRead {
    async fn read_buf(&mut self, dst: &mut bytes::BytesMut) -> std::io::Result<usize>;
}

/// A writable half of an async I/O stream.
#[allow(async_fn_in_trait)]
pub trait NacelleWrite {
    async fn write_all(&mut self, src: &[u8]) -> std::io::Result<()>;
    async fn flush(&mut self) -> std::io::Result<()>;
}

// Under the tokio runtime, blanket-implement both traits for any type that
// already satisfies the equivalent tokio I/O traits.  This covers
// `tokio::io::ReadHalf<IO>` / `WriteHalf<IO>` from `tokio::io::split` as well
// as raw `TcpStream` etc.
#[cfg(feature = "tokio-runtime")]
impl<T: tokio::io::AsyncRead + Unpin> NacelleRead for T {
    async fn read_buf(&mut self, dst: &mut bytes::BytesMut) -> std::io::Result<usize> {
        tokio::io::AsyncReadExt::read_buf(self, dst).await
    }
}

#[cfg(feature = "tokio-runtime")]
impl<T: tokio::io::AsyncWrite + Unpin> NacelleWrite for T {
    async fn write_all(&mut self, src: &[u8]) -> std::io::Result<()> {
        tokio::io::AsyncWriteExt::write_all(self, src).await
    }

    async fn flush(&mut self) -> std::io::Result<()> {
        tokio::io::AsyncWriteExt::flush(self).await
    }
}

// ── tokio (default) ──────────────────────────────────────────────────────────
#[cfg(feature = "tokio-runtime")]
mod tokio_rt;

#[cfg(feature = "tokio-runtime")]
pub use tokio_rt::{JoinError, JoinHandle, spawn};

#[cfg(all(feature = "tcp", feature = "tokio-runtime"))]
pub(crate) use tokio_rt::serve_tcp;

// ── monoio ───────────────────────────────────────────────────────────────────
#[cfg(feature = "monoio-runtime")]
mod monoio_rt;

#[cfg(feature = "monoio-runtime")]
pub use monoio_rt::{JoinError, JoinHandle, spawn};

#[cfg(all(feature = "tcp", feature = "monoio-runtime"))]
pub(crate) use monoio_rt::serve_tcp;
