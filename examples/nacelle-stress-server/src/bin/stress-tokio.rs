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

#[cfg(feature = "tls-self-signed")]
use nacelle::NacelleTlsConfig;
use nacelle::{FrameRequest, Handler, LengthDelimitedProtocol, NacelleError, TcpServer};
use nacelle_stress_common::make_tcp_socket;
use tokio::net::{TcpListener, TcpSocket, TcpStream};
use tokio::sync::watch;
use tokio::time::MissedTickBehavior;

#[cfg(feature = "tls-self-signed")]
type StressTlsConfig = NacelleTlsConfig;
#[cfg(not(feature = "tls-self-signed"))]
type StressTlsConfig = ();

#[cfg(feature = "tls-self-signed")]
fn clone_tls_config(tls_config: &Option<StressTlsConfig>) -> Option<StressTlsConfig> {
    tls_config.clone()
}

#[cfg(not(feature = "tls-self-signed"))]
fn clone_tls_config(tls_config: &Option<StressTlsConfig>) -> Option<StressTlsConfig> {
    *tls_config
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
    let socket = make_tcp_socket(addr)?;
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
    server: TcpServer<FrameRequest, LengthDelimitedProtocol, H>,
    tls_config: Option<StressTlsConfig>,
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
                let tls_config = clone_tls_config(&tls_config);
                let stats = stats.clone();
                stats.record_accepted_connection();
                tokio::spawn(async move {
                    let active_connection = ActiveConnection::new(stats);
                    let result = serve_accepted_stream(server, stream, tls_config).await;
                    active_connection.record_finished(result.is_err());
                });
            }
        }
    }
    Ok(())
}

async fn serve_accepted_stream<H>(
    server: TcpServer<FrameRequest, LengthDelimitedProtocol, H>,
    stream: TcpStream,
    tls_config: Option<StressTlsConfig>,
) -> Result<(), NacelleError>
where
    H: Handler,
{
    #[cfg(feature = "tls-self-signed")]
    if let Some(tls_config) = tls_config {
        let acceptor = tokio_rustls::TlsAcceptor::from(tls_config.server_config());
        let tls_stream =
            tokio::time::timeout(tls_config.handshake_timeout(), acceptor.accept(stream))
                .await
                .map_err(|_| NacelleError::Timeout("tls_handshake"))??;
        return server.serve_io(tls_stream).await;
    }

    #[cfg(not(feature = "tls-self-signed"))]
    let _ = tls_config;

    server.serve_io(stream).await
}

// ---------------------------------------------------------------------------
// Entry point
// ---------------------------------------------------------------------------

fn spawn_server_thread<H>(
    listener: TcpListener,
    server: TcpServer<FrameRequest, LengthDelimitedProtocol, H>,
    tls_config: Option<StressTlsConfig>,
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
        runtime.block_on(run_server(listener, server, tls_config, shutdown, stats))
    })
}

fn build_tls_config(
    config: &shared::ServerConfig,
) -> Result<Option<StressTlsConfig>, Box<dyn std::error::Error + Send + Sync>> {
    #[cfg(feature = "tls-self-signed")]
    {
        if config.tls_self_signed {
            let generated = NacelleTlsConfig::self_signed(["localhost", "127.0.0.1"])?;
            return Ok(Some(generated.tls_config));
        }
        Ok(None)
    }

    #[cfg(not(feature = "tls-self-signed"))]
    {
        if config.tls_self_signed {
            return Err("tokio-server was built without tls-self-signed support".into());
        }
        Ok(None)
    }
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
    let tls_config = build_tls_config(&config)?;

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
        "nacelle-stress-server listening on {} (threads={} transport={})",
        listen_addr,
        n_server_threads,
        if config.tls_self_signed {
            "tcp-tls"
        } else {
            "tcp"
        },
    );

    let (shutdown_tx, shutdown_rx) = watch::channel(false);
    let stats_task = if stats.enabled() {
        Some(tokio::spawn(print_periodic_stats(
            stats.clone(),
            shutdown_rx.clone(),
        )))
    } else {
        None
    };

    if n_server_threads == 1 {
        let server_task = tokio::spawn(run_server(
            first_listener,
            server,
            tls_config,
            shutdown_rx,
            stats.clone(),
        ));
        wait_for_shutdown_signal().await?;
        eprintln!("\nshutting down...");
        let _ = shutdown_tx.send(true);
        server_task.await??;
        if let Some(stats_task) = stats_task {
            stats_task.await?;
        }
        return Ok(());
    }

    let mut server_tasks = Vec::with_capacity(n_server_threads);
    server_tasks.push(spawn_server_thread(
        first_listener,
        server.clone(),
        clone_tls_config(&tls_config),
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
            clone_tls_config(&tls_config),
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
    if let Some(stats_task) = stats_task {
        stats_task.await?;
    }

    Ok(())
}

// ---------------------------------------------------------------------------
// CLI
// ---------------------------------------------------------------------------
