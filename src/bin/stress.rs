#[global_allocator]
static ALLOC: mimalloc::MiMalloc = mimalloc::MiMalloc;

use std::collections::HashMap;
use std::net::SocketAddr;
use std::str::FromStr;
use std::sync::Arc;
use std::time::{Duration, Instant};

use bytes::Bytes;
use nacelle::{
    FRAME_FLAG_END, FRAME_FLAG_ERROR, FrameRequest, LengthDelimitedProtocol, NacelleConfig,
    NacelleError, NacelleServer, RequestBody, ResponseWriter, handler_fn,
};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
use tokio::net::{TcpListener, TcpSocket};
use tokio::sync::{Barrier, watch};

const STRESS_OPCODE: u64 = 1;
const LATENCY_SAMPLE_STRIDE: u64 = 64;

/// Enables the Windows loopback fast-path (SIO_LOOPBACK_FAST_PATH) on a socket.
///
/// This ioctl bypasses most of the Windows TCP/IP stack for loopback connections,
/// reducing per-packet processing from ~300 µs to ~50–100 µs RTT.  It must be
/// called *before* bind() on listening sockets and *before* connect() on client
/// sockets; accepted sockets inherit the setting from the listening socket.
///
/// No-op on non-Windows platforms or Windows versions older than 8 / Server 2012.
#[cfg(windows)]
fn set_loopback_fast_path(raw: std::os::windows::io::RawSocket) {
    // Declare WSAIoctl without pulling in a winapi crate — the symbol is always
    // available via Ws2_32.dll which Tokio already links.
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

    // SAFETY: `raw` is a valid SOCKET from AsRawSocket; WSAIoctl is thread-safe.
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

/// Creates a raw `TcpSocket` with platform-specific low-latency options applied.
///
/// On Windows: sets `SIO_LOOPBACK_FAST_PATH` before any bind/connect.
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

/// Creates a client-side TCP socket ready for `connect()`.
fn make_tcp_socket(addr: &SocketAddr) -> Result<TcpSocket, std::io::Error> {
    new_raw_socket(addr)
}

/// Creates a server-side TCP socket ready for `bind()` + `listen()`.
///
/// When `reuseport` is true and the platform is Linux, `SO_REUSEPORT` is set so
/// that multiple listeners can share the same port with kernel-level load
/// balancing (true thread-per-core accept).  All sockets sharing a port must
/// have `reuseport` enabled — including the very first one.
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

#[derive(Debug, Clone)]
struct StressConfig {
    bind: SocketAddr,
    connections: usize,
    pipeline: usize,
    duration: Duration,
    payload_bytes: usize,
    response_bytes: usize,
    server_threads: usize,
    max_concurrent_requests_per_connection: usize,
    read_buffer_capacity: usize,
    response_buffer_capacity: usize,
    request_body_chunk_size: usize,
    request_body_channel_capacity: usize,
}

impl Default for StressConfig {
    fn default() -> Self {
        Self {
            bind: SocketAddr::from(([127, 0, 0, 1], 0)),
            connections: 256,
            pipeline: 8,
            duration: Duration::from_secs(10),
            payload_bytes: 256,
            response_bytes: 64,
            server_threads: std::thread::available_parallelism()
                .map(|n| n.get())
                .unwrap_or(1),
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

#[derive(Debug, Default)]
struct WorkerResult {
    completed_requests: u64,
    request_wire_bytes: u64,
    response_wire_bytes: u64,
    latency_total_ns: u128,
    max_latency_ns: u64,
    latency_samples_ns: Vec<u64>,
}

#[derive(Debug)]
struct ResponseSummary {
    request_id: u64,
    wire_bytes: usize,
}

#[derive(Debug, Default)]
struct PartialResponse {
    wire_bytes: usize,
}

#[tokio::main(flavor = "multi_thread")]
async fn main() -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let config = parse_args(std::env::args().skip(1))?;
    let server = build_server(&config)?;

    // On non-Linux platforms SO_REUSEPORT is not available; clamp to 1 and warn.
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

    // The first listener binds to the requested address (port 0 → OS assigns a port).
    // Additional listeners (SO_REUSEPORT) must bind to the same concrete port.
    let use_reuseport = n_server_threads > 1;
    let first_socket = make_server_socket(&config.bind, use_reuseport)?;
    first_socket.bind(config.bind)?;
    let first_listener = first_socket.listen(1024)?;
    let listen_addr = first_listener.local_addr()?;

    println!(
        "stress target={} connections={} pipeline={} payload={}B response={}B duration={}s{}",
        listen_addr,
        config.connections,
        config.pipeline,
        config.payload_bytes,
        config.response_bytes,
        config.duration.as_secs_f64(),
        if n_server_threads > 1 {
            format!(" server-threads={n_server_threads}")
        } else {
            String::new()
        },
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

    let barrier = Arc::new(Barrier::new(config.connections + 1));
    let mut workers = Vec::with_capacity(config.connections);
    for worker_id in 0..config.connections {
        workers.push(tokio::spawn(run_worker(
            worker_id,
            listen_addr,
            config.clone(),
            barrier.clone(),
        )));
    }

    barrier.wait().await;
    let load_started = Instant::now();
    let mut aggregate = WorkerResult::default();
    for worker in workers {
        aggregate.merge(worker.await??);
    }
    let total_elapsed = load_started.elapsed();

    let _ = shutdown_tx.send(true);
    for task in server_tasks {
        task.await??;
    }

    print_summary(&config, n_server_threads, &aggregate, total_elapsed);
    Ok(())
}

fn build_server(
    config: &StressConfig,
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
                )
                .with_max_buffered_request_body_per_request(config.payload_bytes.max(1)),
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

async fn run_worker(
    worker_id: usize,
    server_addr: SocketAddr,
    config: StressConfig,
    barrier: Arc<Barrier>,
) -> Result<WorkerResult, NacelleError> {
    let stream = make_tcp_socket(&server_addr)?.connect(server_addr).await?;
    stream.set_nodelay(true)?;
    let (mut reader, mut writer) = stream.into_split();
    let payload = vec![worker_id as u8; config.payload_bytes];
    let mut scratch = Vec::with_capacity(config.response_bytes.max(1024));
    let mut inflight = HashMap::with_capacity(config.pipeline);
    let mut partial_responses = HashMap::<u64, PartialResponse>::with_capacity(config.pipeline);
    let mut result = WorkerResult::default();
    let mut next_request_id = (worker_id as u64) << 48;

    barrier.wait().await;
    let deadline = Instant::now() + config.duration;

    while inflight.len() < config.pipeline {
        if Instant::now() >= deadline {
            break;
        }
        let request_id = next_request_id;
        next_request_id += 1;
        result.request_wire_bytes +=
            write_request_frame(&mut writer, request_id, STRESS_OPCODE, &payload).await? as u64;
        inflight.insert(request_id, Instant::now());
    }

    while !inflight.is_empty() {
        let response = read_response(&mut reader, &mut scratch, &mut partial_responses).await?;
        let started_at =
            inflight
                .remove(&response.request_id)
                .ok_or(NacelleError::InvalidFrame(
                    "response request id was not in the in-flight map",
                ))?;

        let latency_ns = started_at.elapsed().as_nanos().min(u64::MAX as u128) as u64;
        result.record_completion(latency_ns, response.wire_bytes as u64);

        if Instant::now() < deadline {
            let request_id = next_request_id;
            next_request_id += 1;
            result.request_wire_bytes +=
                write_request_frame(&mut writer, request_id, STRESS_OPCODE, &payload).await? as u64;
            inflight.insert(request_id, Instant::now());
        }
    }

    writer.shutdown().await?;
    Ok(result)
}

async fn write_request_frame<W>(
    writer: &mut W,
    request_id: u64,
    opcode: u64,
    payload: &[u8],
) -> Result<usize, NacelleError>
where
    W: AsyncWrite + Unpin,
{
    let frame_len = 20 + payload.len();
    let frame_len = u32::try_from(frame_len).map_err(|_| NacelleError::FrameTooLarge {
        len: frame_len,
        max: u32::MAX as usize,
    })?;

    let mut header = [0_u8; 24];
    header[0..4].copy_from_slice(&frame_len.to_le_bytes());
    header[4..12].copy_from_slice(&request_id.to_le_bytes());
    header[12..20].copy_from_slice(&opcode.to_le_bytes());
    header[20..24].copy_from_slice(&0_u32.to_le_bytes());

    writer.write_all(&header).await?;
    if !payload.is_empty() {
        writer.write_all(payload).await?;
    }

    Ok(header.len() + payload.len())
}

async fn read_response<R>(
    reader: &mut R,
    scratch: &mut Vec<u8>,
    partial_responses: &mut HashMap<u64, PartialResponse>,
) -> Result<ResponseSummary, NacelleError>
where
    R: AsyncRead + Unpin,
{
    loop {
        let (frame_request_id, flags, frame_wire_bytes) = read_frame(reader, scratch).await?;
        if flags & FRAME_FLAG_ERROR != 0 {
            return Err(NacelleError::InvalidFrame(
                "stress target returned an error frame",
            ));
        }

        let partial = partial_responses.entry(frame_request_id).or_default();
        partial.wire_bytes += frame_wire_bytes;
        if flags & FRAME_FLAG_END != 0 {
            let partial =
                partial_responses
                    .remove(&frame_request_id)
                    .ok_or(NacelleError::InvalidFrame(
                        "response completed without partial response state",
                    ))?;
            return Ok(ResponseSummary {
                request_id: frame_request_id,
                wire_bytes: partial.wire_bytes,
            });
        }
    }
}

async fn read_frame<R>(
    reader: &mut R,
    scratch: &mut Vec<u8>,
) -> Result<(u64, u32, usize), NacelleError>
where
    R: AsyncRead + Unpin,
{
    let mut len_buf = [0_u8; 4];
    reader.read_exact(&mut len_buf).await?;
    let frame_len = u32::from_le_bytes(len_buf) as usize;
    if frame_len < 20 {
        return Err(NacelleError::InvalidFrame(
            "stress client received a truncated frame",
        ));
    }

    let mut fixed = [0_u8; 20];
    reader.read_exact(&mut fixed).await?;
    let request_id = u64::from_le_bytes(fixed[0..8].try_into().expect("fixed width"));
    let _opcode = u64::from_le_bytes(fixed[8..16].try_into().expect("fixed width"));
    let flags = u32::from_le_bytes(fixed[16..20].try_into().expect("fixed width"));
    let body_len = frame_len - 20;
    if scratch.len() < body_len {
        scratch.resize(body_len, 0);
    }
    if body_len > 0 {
        reader.read_exact(&mut scratch[..body_len]).await?;
    }

    Ok((request_id, flags, 4 + frame_len))
}

impl WorkerResult {
    fn record_completion(&mut self, latency_ns: u64, response_wire_bytes: u64) {
        self.completed_requests += 1;
        self.response_wire_bytes += response_wire_bytes;
        self.latency_total_ns += latency_ns as u128;
        self.max_latency_ns = self.max_latency_ns.max(latency_ns);
        if self
            .completed_requests
            .is_multiple_of(LATENCY_SAMPLE_STRIDE)
        {
            self.latency_samples_ns.push(latency_ns);
        }
    }

    fn merge(&mut self, other: WorkerResult) {
        self.completed_requests += other.completed_requests;
        self.request_wire_bytes += other.request_wire_bytes;
        self.response_wire_bytes += other.response_wire_bytes;
        self.latency_total_ns += other.latency_total_ns;
        self.max_latency_ns = self.max_latency_ns.max(other.max_latency_ns);
        self.latency_samples_ns.extend(other.latency_samples_ns);
    }
}

fn parse_args(
    args: impl IntoIterator<Item = String>,
) -> Result<StressConfig, Box<dyn std::error::Error + Send + Sync>> {
    let mut config = StressConfig::default();
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
            "--connections" => {
                config.connections = parse_value(&arg, args.next())?;
            }
            "--pipeline" => {
                config.pipeline = parse_value(&arg, args.next())?;
            }
            "--duration-secs" => {
                let secs: f64 = parse_value(&arg, args.next())?;
                config.duration = Duration::from_secs_f64(secs.max(0.1));
            }
            "--payload-bytes" => {
                config.payload_bytes = parse_value(&arg, args.next())?;
            }
            "--response-bytes" => {
                config.response_bytes = parse_value(&arg, args.next())?;
            }
            "--server-threads" => {
                config.server_threads = parse_value(&arg, args.next())?;
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
        return Err("server-threads must be greater than zero".into());
    }
    if config.connections == 0 {
        return Err("connections must be greater than zero".into());
    }
    if config.pipeline == 0 {
        return Err("pipeline must be greater than zero".into());
    }
    if config.max_concurrent_requests_per_connection == 0 {
        return Err("max-concurrent-requests-per-connection must be greater than zero".into());
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
        "Nacelle stress harness\n\
         \n\
         Usage:\n\
           cargo run --release --bin stress -- [options]\n\
         \n\
         Options:\n\
           --bind <addr>                             Bind address for the in-process server (default 127.0.0.1:0)\n\
           --connections <count>                    Concurrent TCP connections (default 256)\n\
           --pipeline <count>                       Requests kept in flight per connection (default 8)\n\
           --duration-secs <seconds>                Active load duration (default 10)\n\
           --payload-bytes <bytes>                  Request payload bytes (default 256)\n\
           --response-bytes <bytes>                 Response payload bytes (default 64)\n\
           --server-threads <count>                 Server accept threads sharing one port via SO_REUSEPORT (default: logical CPU count; Linux only)\n\
           --max-concurrent-requests-per-connection <count>\n\
                                                    Server-side overlap per connection for fully buffered request bodies (default 8)\n\
           --read-buffer <bytes>                    Server read buffer capacity (default 65536)\n\
           --response-buffer <bytes>                Server response encode buffer capacity (default 16384)\n\
           --request-body-chunk-size <bytes>        Request body chunk size (default 16384)\n\
           --request-body-channel-capacity <count>  Request body channel capacity (default 8)\n\
         \n\
         Notes:\n\
           - Connections are established once per worker and reused for the whole run.\n\
           - High pipeline values measure saturation throughput and queueing under load, not just base RTT.\n\
           - For established-connection RTT, start with --pipeline 1.\n\
        "
    );
}

fn print_summary(
    config: &StressConfig,
    n_server_threads: usize,
    result: &WorkerResult,
    elapsed: Duration,
) {
    let elapsed_secs = elapsed.as_secs_f64().max(f64::EPSILON);
    let requests_per_sec = result.completed_requests as f64 / elapsed_secs;
    let total_wire_bytes = result.request_wire_bytes + result.response_wire_bytes;
    let configured_inflight = config.connections.saturating_mul(config.pipeline) as f64;
    let avg_latency_us = if result.completed_requests == 0 {
        0.0
    } else {
        result.latency_total_ns as f64 / result.completed_requests as f64 / 1_000.0
    };
    let little_law_latency_us = if requests_per_sec > 0.0 {
        configured_inflight / requests_per_sec * 1_000_000.0
    } else {
        0.0
    };
    let implied_inflight = requests_per_sec * (avg_latency_us / 1_000_000.0);
    let mut samples = result.latency_samples_ns.clone();
    samples.sort_unstable();

    println!();
    println!("completed_requests   {}", result.completed_requests);
    println!("effective_rps        {:.2}", requests_per_sec);
    println!(
        "wire_throughput      {} total ({} / s)",
        format_bytes(total_wire_bytes),
        format_bytes_per_sec(total_wire_bytes, elapsed)
    );
    println!(
        "request_throughput   {} / s",
        format_bytes_per_sec(result.request_wire_bytes, elapsed)
    );
    println!(
        "response_throughput  {} / s",
        format_bytes_per_sec(result.response_wire_bytes, elapsed)
    );
    println!("avg_latency          {:.2} us", avg_latency_us);
    println!(
        "max_latency          {:.2} us",
        result.max_latency_ns as f64 / 1_000.0
    );
    if !samples.is_empty() {
        println!(
            "p50_latency          {:.2} us",
            percentile(&samples, 0.50) as f64 / 1_000.0
        );
        println!(
            "p95_latency          {:.2} us",
            percentile(&samples, 0.95) as f64 / 1_000.0
        );
        println!(
            "p99_latency          {:.2} us",
            percentile(&samples, 0.99) as f64 / 1_000.0
        );
    }
    println!(
        "configured_inflight  {}",
        config.connections * config.pipeline
    );
    println!("implied_inflight     {:.1}", implied_inflight);
    println!("queue_floor_latency  {:.2} us", little_law_latency_us);
    if config.pipeline > 1 {
        println!(
            "latency_note         saturation mode; use --pipeline 1 for established-connection RTT"
        );
    }
    println!();
    println!(
        "profile              connections={} pipeline={} payload={}B response={}B elapsed={:.2}s{}",
        config.connections,
        config.pipeline,
        config.payload_bytes,
        config.response_bytes,
        elapsed_secs,
        if n_server_threads > 1 {
            format!(" server-threads={n_server_threads}")
        } else {
            String::new()
        },
    );
}

fn percentile(sorted_samples: &[u64], percentile: f64) -> u64 {
    let index = ((sorted_samples.len().saturating_sub(1)) as f64 * percentile).round() as usize;
    sorted_samples[index]
}

fn format_bytes(bytes: u64) -> String {
    const KIB: f64 = 1024.0;
    const MIB: f64 = KIB * 1024.0;
    const GIB: f64 = MIB * 1024.0;

    let bytes_f64 = bytes as f64;
    if bytes_f64 >= GIB {
        format!("{:.2} GiB", bytes_f64 / GIB)
    } else if bytes_f64 >= MIB {
        format!("{:.2} MiB", bytes_f64 / MIB)
    } else if bytes_f64 >= KIB {
        format!("{:.2} KiB", bytes_f64 / KIB)
    } else {
        format!("{bytes} B")
    }
}

fn format_bytes_per_sec(bytes: u64, elapsed: Duration) -> String {
    let per_sec = bytes as f64 / elapsed.as_secs_f64().max(f64::EPSILON);
    format_bytes(per_sec.round() as u64)
}
