use std::collections::VecDeque;
use std::net::SocketAddr;
use std::str::FromStr;
use std::sync::Arc;
use std::time::{Duration, Instant};

use bytes::Bytes;
use cascade::{
    CascadeConfig,
    CascadeError,
    CascadeServer,
    FRAME_FLAG_END,
    FRAME_FLAG_ERROR,
    FrameRequest,
    LengthDelimitedProtocol,
    RequestBody,
    ResponseWriter,
    handler_fn,
};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::{Barrier, watch};

const STRESS_OPCODE: u64 = 1;
const LATENCY_SAMPLE_STRIDE: u64 = 64;

#[derive(Debug, Clone)]
struct StressConfig {
    bind: SocketAddr,
    connections: usize,
    pipeline: usize,
    duration: Duration,
    payload_bytes: usize,
    response_bytes: usize,
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

#[tokio::main(flavor = "multi_thread")]
async fn main() -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let config = parse_args(std::env::args().skip(1))?;
    let server = build_server(&config)?;
    let listener = TcpListener::bind(config.bind).await?;
    let listen_addr = listener.local_addr()?;
    let (shutdown_tx, shutdown_rx) = watch::channel(false);
    let server_task = tokio::spawn(run_server(listener, server, shutdown_rx));

    println!(
        "stress target={} connections={} pipeline={} payload={}B response={}B duration={}s",
        listen_addr,
        config.connections,
        config.pipeline,
        config.payload_bytes,
        config.response_bytes,
        config.duration.as_secs_f64()
    );

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
    server_task.await??;

    print_summary(&config, &aggregate, total_elapsed);
    Ok(())
}

fn build_server(
    config: &StressConfig,
) -> Result<CascadeServer<StressService, FrameRequest, LengthDelimitedProtocol>, CascadeError> {
    CascadeServer::<StressService, FrameRequest, ()>::builder()
        .service(StressService {
            response_payload: Bytes::from(vec![0x5A; config.response_bytes]),
        })
        .protocol(LengthDelimitedProtocol)
        .config(
            CascadeConfig::default()
                .with_read_buffer_capacity(config.read_buffer_capacity)
                .with_response_buffer_capacity(config.response_buffer_capacity)
                .with_request_body_chunk_size(config.request_body_chunk_size)
                .with_request_body_channel_capacity(config.request_body_channel_capacity),
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
                        response.write_bytes(svc.response_payload.clone()).await?;
                    }
                    Ok(())
                },
            ),
        )
        .build()
}

async fn run_server(
    listener: TcpListener,
    server: CascadeServer<StressService, FrameRequest, LengthDelimitedProtocol>,
    mut shutdown: watch::Receiver<bool>,
) -> Result<(), CascadeError> {
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
) -> Result<WorkerResult, CascadeError> {
    let stream = TcpStream::connect(server_addr).await?;
    stream.set_nodelay(true)?;
    let (mut reader, mut writer) = stream.into_split();
    let payload = vec![worker_id as u8; config.payload_bytes];
    let mut scratch = Vec::with_capacity(config.response_bytes.max(1024));
    let mut inflight = VecDeque::with_capacity(config.pipeline);
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
        inflight.push_back((request_id, Instant::now()));
    }

    while let Some((request_id, started_at)) = inflight.pop_front() {
        let response = read_response(&mut reader, &mut scratch).await?;
        if response.request_id != request_id {
            return Err(CascadeError::InvalidFrame("response ordering changed during stress run"));
        }

        let latency_ns = started_at.elapsed().as_nanos().min(u64::MAX as u128) as u64;
        result.record_completion(latency_ns, response.wire_bytes as u64);

        if Instant::now() < deadline {
            let request_id = next_request_id;
            next_request_id += 1;
            result.request_wire_bytes +=
                write_request_frame(&mut writer, request_id, STRESS_OPCODE, &payload).await? as u64;
            inflight.push_back((request_id, Instant::now()));
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
) -> Result<usize, CascadeError>
where
    W: AsyncWrite + Unpin,
{
    let frame_len = 20 + payload.len();
    let frame_len = u32::try_from(frame_len).map_err(|_| CascadeError::FrameTooLarge {
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
) -> Result<ResponseSummary, CascadeError>
where
    R: AsyncRead + Unpin,
{
    let mut request_id = None;
    let mut wire_bytes = 0_usize;

    loop {
        let (frame_request_id, flags, frame_wire_bytes) = read_frame(reader, scratch).await?;
        if flags & FRAME_FLAG_ERROR != 0 {
            return Err(CascadeError::InvalidFrame("stress target returned an error frame"));
        }

        match request_id {
            Some(existing) if existing != frame_request_id => {
                return Err(CascadeError::InvalidFrame("response frames changed request id mid-stream"));
            }
            None => request_id = Some(frame_request_id),
            _ => {}
        }

        wire_bytes += frame_wire_bytes;
        if flags & FRAME_FLAG_END != 0 {
            return Ok(ResponseSummary {
                request_id: request_id.expect("response must include at least one frame"),
                wire_bytes,
            });
        }
    }
}

async fn read_frame<R>(
    reader: &mut R,
    scratch: &mut Vec<u8>,
) -> Result<(u64, u32, usize), CascadeError>
where
    R: AsyncRead + Unpin,
{
    let mut len_buf = [0_u8; 4];
    reader.read_exact(&mut len_buf).await?;
    let frame_len = u32::from_le_bytes(len_buf) as usize;
    if frame_len < 20 {
        return Err(CascadeError::InvalidFrame("stress client received a truncated frame"));
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
        if self.completed_requests % LATENCY_SAMPLE_STRIDE == 0 {
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

    if config.connections == 0 {
        return Err("connections must be greater than zero".into());
    }
    if config.pipeline == 0 {
        return Err("pipeline must be greater than zero".into());
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
        "Cascade stress harness\n\
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
           --read-buffer <bytes>                    Server read buffer capacity (default 65536)\n\
           --response-buffer <bytes>                Server response encode buffer capacity (default 16384)\n\
           --request-body-chunk-size <bytes>        Request body chunk size (default 16384)\n\
           --request-body-channel-capacity <count>  Request body channel capacity (default 8)\n\
        "
    );
}

fn print_summary(config: &StressConfig, result: &WorkerResult, elapsed: Duration) {
    let elapsed_secs = elapsed.as_secs_f64().max(f64::EPSILON);
    let requests_per_sec = result.completed_requests as f64 / elapsed_secs;
    let total_wire_bytes = result.request_wire_bytes + result.response_wire_bytes;
    let avg_latency_us = if result.completed_requests == 0 {
        0.0
    } else {
        result.latency_total_ns as f64 / result.completed_requests as f64 / 1_000.0
    };
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
    println!("max_latency          {:.2} us", result.max_latency_ns as f64 / 1_000.0);
    if !samples.is_empty() {
        println!("p50_latency          {:.2} us", percentile(&samples, 0.50) as f64 / 1_000.0);
        println!("p95_latency          {:.2} us", percentile(&samples, 0.95) as f64 / 1_000.0);
        println!("p99_latency          {:.2} us", percentile(&samples, 0.99) as f64 / 1_000.0);
    }
    println!();
    println!(
        "profile              connections={} pipeline={} payload={}B response={}B elapsed={:.2}s",
        config.connections,
        config.pipeline,
        config.payload_bytes,
        config.response_bytes,
        elapsed_secs
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
