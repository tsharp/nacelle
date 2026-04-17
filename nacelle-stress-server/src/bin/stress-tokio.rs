#[global_allocator]
static ALLOC: mimalloc::MiMalloc = mimalloc::MiMalloc;

#[path = "../shared.rs"]
mod shared;
use shared::{StressService, build_server, parse_args};

use std::net::SocketAddr;

use nacelle::{FrameRequest, LengthDelimitedProtocol, NacelleError, NacelleServer};
use tokio::net::{TcpListener, TcpSocket};
use tokio::sync::watch;

// ---------------------------------------------------------------------------
// Platform-specific low-latency socket helpers
// ---------------------------------------------------------------------------

/// Enables the Windows loopback fast-path (SIO_LOOPBACK_FAST_PATH) on a socket.
#[cfg(windows)]
fn set_loopback_fast_path(raw: std::os::windows::io::RawSocket) {
    unsafe extern "system" {
        fn WSAIoctl(
            s: usize,
            dw_io_control_code: u32,
            lp_vb_in_buffer: *const std::ffi::c_void,
            cb_in_buffer: u32,
            lp_vb_out_buffer: *mut std::ffi::c_void,
            cb_out_buffer: u32,
            lpcb_bytes_returned: *mut u32,
            lp_overlapped: *mut std::ffi::c_void,
            lp_completion_routine: Option<unsafe extern "system" fn()>,
        ) -> i32;
    }

    const SIO_LOOPBACK_FAST_PATH: u32 = 0x9800_0010;
    let enable: u32 = 1;
    let mut bytes_returned: u32 = 0;

    let _ = unsafe {
        WSAIoctl(
            raw as usize,
            SIO_LOOPBACK_FAST_PATH,
            (&enable) as *const u32 as *const std::ffi::c_void,
            std::mem::size_of::<u32>() as u32,
            std::ptr::null_mut(),
            0,
            &mut bytes_returned,
            std::ptr::null_mut(),
            None,
        )
    };
}

fn new_raw_socket(addr: &SocketAddr) -> Result<TcpSocket, std::io::Error> {
    let socket = if addr.is_ipv4() {
        TcpSocket::new_v4()?
    } else {
        TcpSocket::new_v6()?
    };
    #[cfg(windows)]
    {
        use std::os::windows::io::AsRawSocket;
        set_loopback_fast_path(socket.as_raw_socket());
    }
    Ok(socket)
}

/// Creates a server-side TCP socket ready for `bind()` + `listen()`.
///
/// When `reuseport` is true and the platform is Linux, `SO_REUSEPORT` is set so
/// that multiple listeners can share the same port with kernel-level load
/// balancing.
fn make_server_socket(
    addr: &SocketAddr,
    #[cfg_attr(not(target_os = "linux"), allow(unused_variables))] reuseport: bool,
) -> Result<TcpSocket, std::io::Error> {
    let socket = new_raw_socket(addr)?;
    #[cfg(target_os = "linux")]
    if reuseport {
        socket.set_reuseport(true)?;
    }
    Ok(socket)
}

// ---------------------------------------------------------------------------
// Service
// ---------------------------------------------------------------------------

async fn run_server(
    listener: TcpListener,
    server: NacelleServer<StressService, FrameRequest, LengthDelimitedProtocol>,
    mut shutdown: watch::Receiver<bool>,
) -> Result<(), NacelleError> {
    loop {
        tokio::select! {
            changed = shutdown.changed() => {
                if changed.is_err() || *shutdown.borrow() {
                    break;
                }
            }
            accepted = listener.accept() => {
                let (stream, _) = accepted?;
                stream.set_nodelay(true)?;
                let server = server.clone();
                tokio::spawn(async move {
                    let _ = server.serve_io(stream).await;
                });
            }
        }
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Entry point
// ---------------------------------------------------------------------------

#[tokio::main(flavor = "multi_thread")]
async fn main() -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let config = parse_args(std::env::args().skip(1), "tokio")?;
    let server = build_server(&config)?;

    #[cfg(not(target_os = "linux"))]
    let n_server_threads = {
        if config.server_threads > 1 {
            eprintln!(
                "note: --server-threads > 1 requires Linux SO_REUSEPORT; using 1 server thread"
            );
        }
        1_usize
    };
    #[cfg(target_os = "linux")]
    let n_server_threads = config.server_threads.max(1);

    let use_reuseport = n_server_threads > 1;
    let first_socket = make_server_socket(&config.bind, use_reuseport)?;
    first_socket.bind(config.bind)?;
    let first_listener = first_socket.listen(1024)?;
    let listen_addr = first_listener.local_addr()?;

    println!(
        "nacelle-stress-server listening on {} (threads={})",
        listen_addr, n_server_threads,
    );

    let (shutdown_tx, shutdown_rx) = watch::channel(false);

    let mut server_tasks = Vec::with_capacity(n_server_threads);
    server_tasks.push(tokio::spawn(run_server(
        first_listener,
        server.clone(),
        shutdown_rx.clone(),
    )));
    for _ in 1..n_server_threads {
        let socket = make_server_socket(&listen_addr, true)?;
        socket.bind(listen_addr)?;
        let listener = socket.listen(1024)?;
        server_tasks.push(tokio::spawn(run_server(
            listener,
            server.clone(),
            shutdown_rx.clone(),
        )));
    }

    tokio::signal::ctrl_c().await?;
    eprintln!("\nshutting down…");
    let _ = shutdown_tx.send(true);
    for task in server_tasks {
        task.await??;
    }

    Ok(())
}

// ---------------------------------------------------------------------------
// CLI
// ---------------------------------------------------------------------------
