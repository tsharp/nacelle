#[global_allocator]
static ALLOC: mimalloc::MiMalloc = mimalloc::MiMalloc;

use std::net::SocketAddr;
use std::str::FromStr;
use std::sync::Arc;

use bytes::Bytes;
use nacelle::{
    FrameRequest, LengthDelimitedProtocol, NacelleConfig, NacelleError, NacelleServer, RequestBody,
    ResponseWriter, handler_fn,
};
use tokio::net::{TcpListener, TcpSocket};
use tokio::sync::watch;

const STRESS_OPCODE: u64 = 1;

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

#[derive(Debug, Clone)]
struct ServerConfig {
    bind: SocketAddr,
    server_threads: usize,
    response_bytes: usize,
    max_concurrent_requests_per_connection: usize,
    read_buffer_capacity: usize,
    response_buffer_capacity: usize,
    request_body_chunk_size: usize,
    request_body_channel_capacity: usize,
}

impl Default for ServerConfig {
    fn default() -> Self {
        Self {
            bind: SocketAddr::from(([127, 0, 0, 1], 7878)),
            server_threads: std::thread::available_parallelism()
                .map(|n| n.get())
                .unwrap_or(1),
            response_bytes: 64,
            max_concurrent_requests_per_connection: 8,
            read_buffer_capacity: 64 * 1024,
            response_buffer_capacity: 16 * 1024,
            request_body_chunk_size: 16 * 1024,
            request_body_channel_capacity: 8,
        }
    }
}

#[derive(Debug)]
struct StressService {
    response_payload: Bytes,
}

fn build_server(
    config: &ServerConfig,
) -> Result<NacelleServer<StressService, FrameRequest, LengthDelimitedProtocol>, NacelleError> {
    NacelleServer::<StressService, FrameRequest, ()>::builder()
        .service(StressService {
            response_payload: Bytes::from(vec![0x5A; config.response_bytes]),
        })
        .protocol(LengthDelimitedProtocol)
        .config(
            NacelleConfig::default()
                .with_read_buffer_capacity(config.read_buffer_capacity)
                .with_response_buffer_capacity(config.response_buffer_capacity)
                .with_request_body_chunk_size(config.request_body_chunk_size)
                .with_request_body_channel_capacity(config.request_body_channel_capacity)
                .with_max_concurrent_requests_per_connection(
                    config.max_concurrent_requests_per_connection,
                ),
        )
        .register_handler(
            STRESS_OPCODE,
            handler_fn(
                |svc: Arc<StressService>,
                 _req: FrameRequest,
                 mut body: RequestBody,
                 response: ResponseWriter| async move {
                    while let Some(chunk) = body.next_chunk().await {
                        let _ = chunk?;
                    }
                    if !svc.response_payload.is_empty() {
                        response.write_bytes(svc.response_payload.clone())?;
                    }
                    Ok(())
                },
            ),
        )
        .build()
}

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
    let config = parse_args(std::env::args().skip(1))?;
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

fn parse_args(
    args: impl IntoIterator<Item = String>,
) -> Result<ServerConfig, Box<dyn std::error::Error + Send + Sync>> {
    let mut config = ServerConfig::default();
    let mut args = args.into_iter();

    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--help" | "-h" => {
                print_help();
                std::process::exit(0);
            }
            "--bind" => {
                config.bind = parse_value(&arg, args.next())?;
            }
            "--server-threads" => {
                config.server_threads = parse_value(&arg, args.next())?;
            }
            "--response-bytes" => {
                config.response_bytes = parse_value(&arg, args.next())?;
            }
            "--max-concurrent-requests-per-connection" => {
                config.max_concurrent_requests_per_connection = parse_value(&arg, args.next())?;
            }
            "--read-buffer" => {
                config.read_buffer_capacity = parse_value(&arg, args.next())?;
            }
            "--response-buffer" => {
                config.response_buffer_capacity = parse_value(&arg, args.next())?;
            }
            "--request-body-chunk-size" => {
                config.request_body_chunk_size = parse_value(&arg, args.next())?;
            }
            "--request-body-channel-capacity" => {
                config.request_body_channel_capacity = parse_value(&arg, args.next())?;
            }
            other => {
                return Err(format!("unknown argument: {other}").into());
            }
        }
    }

    if config.server_threads == 0 {
        return Err("--server-threads must be greater than zero".into());
    }

    Ok(config)
}

fn parse_value<T>(
    flag: &str,
    value: Option<String>,
) -> Result<T, Box<dyn std::error::Error + Send + Sync>>
where
    T: FromStr,
    T::Err: std::error::Error + Send + Sync + 'static,
{
    let value = value.ok_or_else(|| format!("missing value for {flag}"))?;
    Ok(value.parse::<T>()?)
}

fn print_help() {
    println!(
        "Nacelle stress server\n\
         \n\
         Runs a standalone nacelle server that can be targeted by an external\n\
         load generator (e.g. nacelle-stress-test with --bind pointing here).\n\
         \n\
         Usage:\n\
           cargo run --release --bin nacelle-stress-server -- [options]\n\
         \n\
         Options:\n\
           --bind <addr>                             Listen address (default 127.0.0.1:7878)\n\
           --server-threads <count>                 Accept threads sharing one port via SO_REUSEPORT (default: logical CPUs; Linux only)\n\
           --response-bytes <bytes>                 Response payload bytes per request (default 64)\n\
           --max-concurrent-requests-per-connection <count>\n\
                                                    Server-side overlap per connection (default 8)\n\
           --read-buffer <bytes>                    Read buffer capacity (default 65536)\n\
           --response-buffer <bytes>                Response encode buffer capacity (default 16384)\n\
           --request-body-chunk-size <bytes>        Request body chunk size (default 16384)\n\
           --request-body-channel-capacity <count>  Request body channel capacity (default 8)\n\
        "
    );
}
