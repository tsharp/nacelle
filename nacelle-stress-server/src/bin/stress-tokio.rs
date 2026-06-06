#[global_allocator]
static ALLOC: mimalloc::MiMalloc = mimalloc::MiMalloc;

#[path = "../shared.rs"]
mod shared;
use shared::{
    StressServerStats, StressServerStatsSnapshot, build_server, configure_allocator, parse_args,
    print_config,
};

use std::net::SocketAddr;
use std::sync::Arc;
use std::thread;
use std::time::Duration;

use nacelle::{FrameRequest, Handler, LengthDelimitedProtocol, NacelleError, RawTcpServer};
use tokio::net::{TcpListener, TcpSocket};
use tokio::sync::watch;
use tokio::time::MissedTickBehavior;

const SOCKET_BUFFER_BYTES: u32 = 4 * 1024 * 1024;

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
    socket.set_recv_buffer_size(SOCKET_BUFFER_BYTES)?;
    socket.set_send_buffer_size(SOCKET_BUFFER_BYTES)?;
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

async fn run_server<H>(
    listener: TcpListener,
    server: RawTcpServer<FrameRequest, LengthDelimitedProtocol, H>,
    mut shutdown: watch::Receiver<bool>,
    stats: Arc<StressServerStats>,
) -> Result<(), NacelleError>
where
    H: Handler,
{
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
                let stats = stats.clone();
                stats.record_accepted_connection();
                tokio::spawn(async move {
                    let active_connection = ActiveConnection::new(stats);
                    let result = server.serve_io(stream).await;
                    active_connection.record_finished(result.is_err());
                });
            }
        }
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Entry point
// ---------------------------------------------------------------------------

fn spawn_server_thread<H>(
    listener: TcpListener,
    server: RawTcpServer<FrameRequest, LengthDelimitedProtocol, H>,
    shutdown: watch::Receiver<bool>,
    stats: Arc<StressServerStats>,
) -> thread::JoinHandle<Result<(), NacelleError>>
where
    H: Handler,
{
    thread::spawn(move || {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .map_err(NacelleError::from)?;
        runtime.block_on(run_server(listener, server, shutdown, stats))
    })
}

// ---------------------------------------------------------------------------
// Stats
// ---------------------------------------------------------------------------

struct ActiveConnection {
    stats: Arc<StressServerStats>,
}

impl ActiveConnection {
    fn new(stats: Arc<StressServerStats>) -> Self {
        Self { stats }
    }

    fn record_finished(&self, failed: bool) {
        self.stats.record_finished_connection(failed);
    }
}

impl Drop for ActiveConnection {
    fn drop(&mut self) {
        self.stats.record_inactive_connection();
    }
}

async fn print_periodic_stats(stats: Arc<StressServerStats>, mut shutdown: watch::Receiver<bool>) {
    let mut interval = tokio::time::interval(Duration::from_secs(5));
    interval.set_missed_tick_behavior(MissedTickBehavior::Skip);
    interval.tick().await;
    let mut previous = stats.snapshot();

    loop {
        tokio::select! {
            changed = shutdown.changed() => {
                if changed.is_err() || *shutdown.borrow() {
                    break;
                }
            }
            _ = interval.tick() => {
                let snapshot = stats.snapshot();
                print_server_stats("periodic", snapshot, Some(previous));
                previous = snapshot;
            }
        }
    }
}

fn print_server_stats(
    kind: &str,
    snapshot: StressServerStatsSnapshot,
    previous: Option<StressServerStatsSnapshot>,
) {
    let completed_connections_per_sec = previous
        .map(|previous| {
            let elapsed = snapshot
                .uptime
                .saturating_sub(previous.uptime)
                .as_secs_f64()
                .max(f64::EPSILON);
            (snapshot
                .completed_connections
                .saturating_sub(previous.completed_connections)) as f64
                / elapsed
        })
        .unwrap_or_else(|| {
            snapshot.completed_connections as f64 / snapshot.uptime.as_secs_f64().max(f64::EPSILON)
        });
    let completed_requests_per_sec = previous
        .map(|previous| {
            let elapsed = snapshot
                .uptime
                .saturating_sub(previous.uptime)
                .as_secs_f64()
                .max(f64::EPSILON);
            (snapshot
                .completed_requests
                .saturating_sub(previous.completed_requests)) as f64
                / elapsed
        })
        .unwrap_or_else(|| {
            snapshot.completed_requests as f64 / snapshot.uptime.as_secs_f64().max(f64::EPSILON)
        });
    let request_mib_per_sec = previous
        .map(|previous| {
            let elapsed = snapshot
                .uptime
                .saturating_sub(previous.uptime)
                .as_secs_f64()
                .max(f64::EPSILON);
            bytes_to_mib(
                snapshot
                    .request_body_bytes
                    .saturating_sub(previous.request_body_bytes),
            ) / elapsed
        })
        .unwrap_or_else(|| {
            bytes_to_mib(snapshot.request_body_bytes)
                / snapshot.uptime.as_secs_f64().max(f64::EPSILON)
        });
    let response_mib_per_sec = previous
        .map(|previous| {
            let elapsed = snapshot
                .uptime
                .saturating_sub(previous.uptime)
                .as_secs_f64()
                .max(f64::EPSILON);
            bytes_to_mib(
                snapshot
                    .response_body_bytes
                    .saturating_sub(previous.response_body_bytes),
            ) / elapsed
        })
        .unwrap_or_else(|| {
            bytes_to_mib(snapshot.response_body_bytes)
                / snapshot.uptime.as_secs_f64().max(f64::EPSILON)
        });
    let accepted_delta = previous
        .map(|previous| {
            snapshot
                .accepted_connections
                .saturating_sub(previous.accepted_connections)
        })
        .unwrap_or(snapshot.accepted_connections);
    let completed_delta = previous
        .map(|previous| {
            snapshot
                .completed_connections
                .saturating_sub(previous.completed_connections)
        })
        .unwrap_or(snapshot.completed_connections);
    let failed_delta = previous
        .map(|previous| {
            snapshot
                .failed_connections
                .saturating_sub(previous.failed_connections)
        })
        .unwrap_or(snapshot.failed_connections);
    let completed_request_delta = previous
        .map(|previous| {
            snapshot
                .completed_requests
                .saturating_sub(previous.completed_requests)
        })
        .unwrap_or(snapshot.completed_requests);
    let failed_request_delta = previous
        .map(|previous| {
            snapshot
                .failed_requests
                .saturating_sub(previous.failed_requests)
        })
        .unwrap_or(snapshot.failed_requests);

    println!(
        "nacelle-stress-server stats kind={} uptime={:.1}s active_connections={} accepted_connections_total={} accepted_connections_delta={} completed_connections_total={} completed_connections_delta={} failed_connections_total={} failed_connections_delta={} completed_connections_per_sec={:.2} active_requests={} completed_requests_total={} completed_requests_delta={} failed_requests_total={} failed_requests_delta={} completed_requests_per_sec={:.2} request_body_mib_per_sec={:.2} response_body_mib_per_sec={:.2}",
        kind,
        snapshot.uptime.as_secs_f64(),
        snapshot.active_connections,
        snapshot.accepted_connections,
        accepted_delta,
        snapshot.completed_connections,
        completed_delta,
        snapshot.failed_connections,
        failed_delta,
        completed_connections_per_sec,
        snapshot.active_requests,
        snapshot.completed_requests,
        completed_request_delta,
        snapshot.failed_requests,
        failed_request_delta,
        completed_requests_per_sec,
        request_mib_per_sec,
        response_mib_per_sec,
    );
}

fn bytes_to_mib(bytes: u64) -> f64 {
    bytes as f64 / 1024.0 / 1024.0
}

async fn wait_for_shutdown_signal() -> Result<(), std::io::Error> {
    #[cfg(unix)]
    {
        let mut terminate =
            tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())?;
        tokio::select! {
            result = tokio::signal::ctrl_c() => result,
            _ = terminate.recv() => Ok(()),
        }
    }

    #[cfg(not(unix))]
    {
        tokio::signal::ctrl_c().await
    }
}

#[tokio::main(flavor = "multi_thread")]
async fn main() -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let config = parse_args(std::env::args().skip(1), "tokio")?;
    configure_allocator(config.low_memory);
    let stats = Arc::new(StressServerStats::new(config.stats_enabled));
    let server = build_server(&config, stats.clone())?;

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

    print_config(&config, "tokio", n_server_threads);

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
    let stats_task = tokio::spawn(print_periodic_stats(stats.clone(), shutdown_rx.clone()));

    if n_server_threads == 1 {
        let server_task = tokio::spawn(run_server(
            first_listener,
            server,
            shutdown_rx,
            stats.clone(),
        ));
        wait_for_shutdown_signal().await?;
        eprintln!("\nshutting down...");
        let _ = shutdown_tx.send(true);
        server_task.await??;
        stats_task.await?;
        print_server_stats("final", stats.snapshot(), None);
        return Ok(());
    }

    let mut server_tasks = Vec::with_capacity(n_server_threads);
    server_tasks.push(spawn_server_thread(
        first_listener,
        server.clone(),
        shutdown_rx.clone(),
        stats.clone(),
    ));
    for _ in 1..n_server_threads {
        let socket = make_server_socket(&listen_addr, true)?;
        socket.bind(listen_addr)?;
        let listener = socket.listen(1024)?;
        server_tasks.push(spawn_server_thread(
            listener,
            server.clone(),
            shutdown_rx.clone(),
            stats.clone(),
        ));
    }

    wait_for_shutdown_signal().await?;
    eprintln!("\nshutting down...");
    let _ = shutdown_tx.send(true);
    for task in server_tasks {
        task.join()
            .map_err(|_| "server thread panicked")?
            .map_err(|error| -> Box<dyn std::error::Error + Send + Sync> { Box::new(error) })?;
    }
    stats_task.await?;
    print_server_stats("final", stats.snapshot(), None);

    Ok(())
}

// ---------------------------------------------------------------------------
// CLI
// ---------------------------------------------------------------------------
