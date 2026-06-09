// Shared service/config logic for the Tokio stress server.

use std::net::SocketAddr;
use std::os::raw::c_long;
use std::path::Path;
use std::str::FromStr;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;
use std::time::Instant;

use bytes::Bytes;
use nacelle::{
    FrameRequest, Handler, LengthDelimitedProtocol, NacelleConfig, NacelleError, NacelleRequest,
    NacelleResponse, NacelleRuntimeState, TcpServer, handler_fn,
};
use nacelle_stress_common::STRESS_OPCODE;
use serde::Deserialize;

const DEFAULT_CONFIG_PATH: &str = "config.toml";

#[derive(Debug)]
pub struct StressServerStats {
    enabled: bool,
    started_at: Instant,
    accepted_connections: AtomicU64,
    active_connections: AtomicU64,
    completed_connections: AtomicU64,
    failed_connections: AtomicU64,
    active_requests: AtomicU64,
    completed_requests: AtomicU64,
    failed_requests: AtomicU64,
    request_body_bytes: AtomicU64,
    response_body_bytes: AtomicU64,
}

#[derive(Debug, Clone, Copy)]
pub struct StressServerStatsSnapshot {
    pub uptime: Duration,
    pub accepted_connections: u64,
    pub active_connections: u64,
    pub completed_connections: u64,
    pub failed_connections: u64,
    pub active_requests: u64,
    pub completed_requests: u64,
    pub failed_requests: u64,
    pub request_body_bytes: u64,
    pub response_body_bytes: u64,
}

impl Default for StressServerStats {
    fn default() -> Self {
        Self::new(false)
    }
}

impl StressServerStats {
    pub fn new(enabled: bool) -> Self {
        Self {
            enabled,
            started_at: Instant::now(),
            accepted_connections: AtomicU64::new(0),
            active_connections: AtomicU64::new(0),
            completed_connections: AtomicU64::new(0),
            failed_connections: AtomicU64::new(0),
            active_requests: AtomicU64::new(0),
            completed_requests: AtomicU64::new(0),
            failed_requests: AtomicU64::new(0),
            request_body_bytes: AtomicU64::new(0),
            response_body_bytes: AtomicU64::new(0),
        }
    }

    pub fn enabled(&self) -> bool {
        self.enabled
    }

    pub fn record_accepted_connection(&self) {
        if !self.enabled {
            return;
        }
        self.accepted_connections.fetch_add(1, Ordering::Relaxed);
        self.active_connections.fetch_add(1, Ordering::Relaxed);
    }

    pub fn record_finished_connection(&self, failed: bool) {
        if !self.enabled {
            return;
        }
        self.completed_connections.fetch_add(1, Ordering::Relaxed);
        if failed {
            self.failed_connections.fetch_add(1, Ordering::Relaxed);
        }
    }

    pub fn record_inactive_connection(&self) {
        if !self.enabled {
            return;
        }
        self.active_connections.fetch_sub(1, Ordering::Relaxed);
    }

    pub fn start_request(self: &Arc<Self>) -> ActiveRequest {
        self.active_requests.fetch_add(1, Ordering::Relaxed);
        ActiveRequest {
            stats: self.clone(),
            finished: false,
        }
    }

    pub fn snapshot(&self) -> StressServerStatsSnapshot {
        StressServerStatsSnapshot {
            uptime: self.started_at.elapsed(),
            accepted_connections: self.accepted_connections.load(Ordering::Relaxed),
            active_connections: self.active_connections.load(Ordering::Relaxed),
            completed_connections: self.completed_connections.load(Ordering::Relaxed),
            failed_connections: self.failed_connections.load(Ordering::Relaxed),
            active_requests: self.active_requests.load(Ordering::Relaxed),
            completed_requests: self.completed_requests.load(Ordering::Relaxed),
            failed_requests: self.failed_requests.load(Ordering::Relaxed),
            request_body_bytes: self.request_body_bytes.load(Ordering::Relaxed),
            response_body_bytes: self.response_body_bytes.load(Ordering::Relaxed),
        }
    }

    fn record_finished_request(&self, failed: bool, request_bytes: u64, response_bytes: u64) {
        if !self.enabled {
            return;
        }
        if failed {
            self.failed_requests.fetch_add(1, Ordering::Relaxed);
        } else {
            self.completed_requests.fetch_add(1, Ordering::Relaxed);
        }
        self.request_body_bytes
            .fetch_add(request_bytes, Ordering::Relaxed);
        self.response_body_bytes
            .fetch_add(response_bytes, Ordering::Relaxed);
    }

    fn record_inactive_request(&self) {
        if !self.enabled {
            return;
        }
        self.active_requests.fetch_sub(1, Ordering::Relaxed);
    }
}

pub struct ActiveRequest {
    stats: Arc<StressServerStats>,
    finished: bool,
}

impl ActiveRequest {
    pub fn record_completed(mut self, request_bytes: u64, response_bytes: u64) {
        self.stats
            .record_finished_request(false, request_bytes, response_bytes);
        self.finished = true;
    }

    pub fn record_failed(mut self, request_bytes: u64) {
        self.stats.record_finished_request(true, request_bytes, 0);
        self.finished = true;
    }
}

impl Drop for ActiveRequest {
    fn drop(&mut self) {
        if !self.finished {
            self.stats.record_finished_request(true, 0, 0);
        }
        self.stats.record_inactive_request();
    }
}

#[derive(Debug, Clone)]
pub struct ServerConfig {
    pub config_sources: Vec<String>,
    pub bind: SocketAddr,
    pub server_threads: usize,
    pub response_bytes: usize,
    pub read_buffer_capacity: usize,
    pub response_buffer_capacity: usize,
    pub request_body_chunk_size: usize,
    pub request_body_channel_capacity: usize,
    pub low_memory: bool,
    pub stats_enabled: bool,
    pub tls_self_signed: bool,
    pub limits: nacelle::NacelleLimits,
}

impl Default for ServerConfig {
    fn default() -> Self {
        Self {
            config_sources: vec!["code defaults".to_string()],
            bind: SocketAddr::from(([127, 0, 0, 1], 7878)),
            server_threads: std::thread::available_parallelism()
                .map(|n| n.get())
                .unwrap_or(1),
            response_bytes: 64,
            read_buffer_capacity: 64 * 1024,
            response_buffer_capacity: 16 * 1024,
            request_body_chunk_size: 16 * 1024,
            request_body_channel_capacity: 8,
            low_memory: false,
            stats_enabled: false,
            tls_self_signed: false,
            limits: nacelle::NacelleLimits::default(),
        }
    }
}

#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct ServerConfigFile {
    bind: Option<SocketAddr>,
    server_threads: Option<usize>,
    response_bytes: Option<usize>,
    read_buffer_capacity: Option<usize>,
    response_buffer_capacity: Option<usize>,
    request_body_chunk_size: Option<usize>,
    request_body_channel_capacity: Option<usize>,
    low_memory: Option<bool>,
    stats_enabled: Option<bool>,
    tls_self_signed: Option<bool>,
    limits: Option<LimitsConfigFile>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct LimitsConfigFile {
    max_connections: Option<usize>,
    max_connections_per_peer: Option<usize>,
    max_connection_opens_per_peer_per_second: Option<usize>,
    max_in_flight_requests: Option<usize>,
    max_streaming_tasks: Option<usize>,
    max_memory_bytes: Option<usize>,
    max_request_body_bytes: Option<usize>,
    max_response_body_bytes: Option<usize>,
    read_timeout_ms: Option<u64>,
    write_timeout_ms: Option<u64>,
    handler_timeout_ms: Option<u64>,
    idle_timeout_ms: Option<u64>,
    http_header_read_timeout_ms: Option<u64>,
    http_request_body_read_timeout_ms: Option<u64>,
    http_response_write_timeout_ms: Option<u64>,
    http_keep_alive: Option<bool>,
    http_max_connection_age_ms: Option<u64>,
}

impl ServerConfig {
    fn apply_file(
        &mut self,
        path: impl AsRef<Path>,
    ) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        let path = path.as_ref();
        let config = std::fs::read_to_string(path)?;
        let file = toml::from_str::<ServerConfigFile>(&config)?;
        self.apply_config_file(file);
        self.config_sources.push(format!("toml {}", path.display()));
        Ok(())
    }

    fn apply_config_file(&mut self, file: ServerConfigFile) {
        if let Some(bind) = file.bind {
            self.bind = bind;
        }
        if let Some(server_threads) = file.server_threads {
            self.server_threads = server_threads;
        }
        if let Some(response_bytes) = file.response_bytes {
            self.response_bytes = response_bytes;
        }
        if let Some(read_buffer_capacity) = file.read_buffer_capacity {
            self.read_buffer_capacity = read_buffer_capacity;
        }
        if let Some(response_buffer_capacity) = file.response_buffer_capacity {
            self.response_buffer_capacity = response_buffer_capacity;
        }
        if let Some(request_body_chunk_size) = file.request_body_chunk_size {
            self.request_body_chunk_size = request_body_chunk_size;
        }
        if let Some(request_body_channel_capacity) = file.request_body_channel_capacity {
            self.request_body_channel_capacity = request_body_channel_capacity;
        }
        if let Some(low_memory) = file.low_memory {
            self.low_memory = low_memory;
        }
        if let Some(stats_enabled) = file.stats_enabled {
            self.stats_enabled = stats_enabled;
        }
        if let Some(tls_self_signed) = file.tls_self_signed {
            self.tls_self_signed = tls_self_signed;
        }
        if let Some(limits) = file.limits {
            self.apply_limits_file(limits);
        }
    }

    fn apply_limits_file(&mut self, file: LimitsConfigFile) {
        if let Some(max_connections) = file.max_connections {
            self.limits.max_connections = max_connections.max(1);
        }
        if let Some(max_connections_per_peer) = file.max_connections_per_peer {
            self.limits.max_connections_per_peer = Some(max_connections_per_peer.max(1));
        }
        if let Some(max) = file.max_connection_opens_per_peer_per_second {
            self.limits.max_connection_opens_per_peer_per_second = Some(max.max(1));
        }
        if let Some(max_in_flight_requests) = file.max_in_flight_requests {
            self.limits.max_in_flight_requests = max_in_flight_requests.max(1);
        }
        if let Some(max_streaming_tasks) = file.max_streaming_tasks {
            self.limits.max_streaming_tasks = max_streaming_tasks.max(1);
        }
        if let Some(max_memory_bytes) = file.max_memory_bytes {
            self.limits.max_memory_bytes = max_memory_bytes.max(1);
        }
        if let Some(max_request_body_bytes) = file.max_request_body_bytes {
            self.limits.max_request_body_bytes = max_request_body_bytes;
        }
        if let Some(max_response_body_bytes) = file.max_response_body_bytes {
            self.limits.max_response_body_bytes = max_response_body_bytes;
        }
        if let Some(read_timeout_ms) = file.read_timeout_ms {
            self.limits.read_timeout = Some(Duration::from_millis(read_timeout_ms));
        }
        if let Some(write_timeout_ms) = file.write_timeout_ms {
            self.limits.write_timeout = Some(Duration::from_millis(write_timeout_ms));
        }
        if let Some(handler_timeout_ms) = file.handler_timeout_ms {
            self.limits.handler_timeout = Some(Duration::from_millis(handler_timeout_ms));
        }
        if let Some(idle_timeout_ms) = file.idle_timeout_ms {
            self.limits.idle_timeout = Some(Duration::from_millis(idle_timeout_ms));
        }
        if let Some(http_header_read_timeout_ms) = file.http_header_read_timeout_ms {
            self.limits.http_header_read_timeout =
                Some(Duration::from_millis(http_header_read_timeout_ms));
        }
        if let Some(http_request_body_read_timeout_ms) = file.http_request_body_read_timeout_ms {
            self.limits.http_request_body_read_timeout =
                Some(Duration::from_millis(http_request_body_read_timeout_ms));
        }
        if let Some(http_response_write_timeout_ms) = file.http_response_write_timeout_ms {
            self.limits.http_response_write_timeout =
                Some(Duration::from_millis(http_response_write_timeout_ms));
        }
        if let Some(http_keep_alive) = file.http_keep_alive {
            self.limits.http_keep_alive = http_keep_alive;
        }
        if let Some(http_max_connection_age_ms) = file.http_max_connection_age_ms {
            self.limits.http_max_connection_age =
                Some(Duration::from_millis(http_max_connection_age_ms));
        }
    }
}

/// mimalloc v2 option constants not yet exposed by `libmimalloc-sys`.
/// Values match the `mi_option_e` enum in mimalloc v2 `mimalloc/types.h`.
/// These are stable within v2 and `libmimalloc-sys` always compiles v2 unless
/// the `v3` feature is explicitly enabled.
const MI_OPTION_ARENA_EAGER_COMMIT: libmimalloc_sys::mi_option_t = 4;
const MI_OPTION_PURGE_DELAY: libmimalloc_sys::mi_option_t = 15;

/// Configures mimalloc for minimal OS memory retention when `low_memory` is
/// true.  Call this once at the start of `main()`, before spawning threads.
///
/// The same effect can be achieved without recompiling by setting environment
/// variables before launching the server:
///   `MIMALLOC_PURGE_DELAY=0 MIMALLOC_ARENA_EAGER_COMMIT=0 ./server`
pub fn configure_allocator(low_memory: bool) {
    if !low_memory {
        return;
    }
    unsafe {
        // Return freed pages to the OS immediately instead of holding them for
        // a default grace period (~100 ms).  Reduces RSS after traffic spikes.
        libmimalloc_sys::mi_option_set(MI_OPTION_PURGE_DELAY, 0 as c_long);
        // Do not pre-commit arena memory up-front; commit only as needed.
        libmimalloc_sys::mi_option_set(MI_OPTION_ARENA_EAGER_COMMIT, 0 as c_long);
    }
}

pub fn build_server(
    config: &ServerConfig,
    stats: Arc<StressServerStats>,
) -> Result<TcpServer<FrameRequest, LengthDelimitedProtocol, impl Handler>, NacelleError> {
    let response_payload = Bytes::from(vec![0x5A; config.response_bytes]);
    let stats_enabled = stats.enabled();
    TcpServer::<FrameRequest, ()>::builder()
        .protocol(LengthDelimitedProtocol)
        .config(
            NacelleConfig::default()
                .with_read_buffer_capacity(config.read_buffer_capacity)
                .with_response_buffer_capacity(config.response_buffer_capacity)
                .with_request_body_chunk_size(config.request_body_chunk_size)
                .with_request_body_channel_capacity(config.request_body_channel_capacity),
        )
        .runtime_state(NacelleRuntimeState::new(config.limits.clone()))
        .handler(handler_fn(move |mut request: NacelleRequest| {
            let response_payload = response_payload.clone();
            let stats = stats.clone();
            async move {
                if !stats_enabled {
                    let opcode = request.tcp_opcode().unwrap_or_default();
                    while let Some(chunk) = request.body.next_chunk().await {
                        let _ = chunk?;
                    }
                    if opcode != STRESS_OPCODE {
                        return Err(NacelleError::handler(std::io::Error::other(format!(
                            "unknown opcode {}",
                            opcode
                        ))));
                    }
                    return Ok(NacelleResponse::tcp_bytes(response_payload));
                }

                let request_stats = stats.start_request();
                let opcode = request.tcp_opcode().unwrap_or_default();
                let mut request_body_bytes = 0_u64;
                while let Some(chunk) = request.body.next_chunk().await {
                    let chunk = match chunk {
                        Ok(chunk) => chunk,
                        Err(error) => {
                            request_stats.record_failed(request_body_bytes);
                            return Err(error);
                        }
                    };
                    request_body_bytes = request_body_bytes.saturating_add(chunk.len() as u64);
                }
                if opcode != STRESS_OPCODE {
                    request_stats.record_failed(request_body_bytes);
                    return Err(NacelleError::handler(std::io::Error::other(format!(
                        "unknown opcode {}",
                        opcode
                    ))));
                }
                request_stats.record_completed(request_body_bytes, response_payload.len() as u64);
                Ok(NacelleResponse::tcp_bytes(response_payload))
            }
        }))
        .build()
}

pub fn parse_args(
    args: impl IntoIterator<Item = String>,
    runtime: &str,
) -> Result<ServerConfig, Box<dyn std::error::Error + Send + Sync>> {
    parse_args_with_default_config(args, runtime, Path::new(DEFAULT_CONFIG_PATH))
}

fn parse_args_with_default_config(
    args: impl IntoIterator<Item = String>,
    runtime: &str,
    default_config_path: &Path,
) -> Result<ServerConfig, Box<dyn std::error::Error + Send + Sync>> {
    let args = args.into_iter().collect::<Vec<_>>();
    if args.iter().any(|arg| arg == "--help" || arg == "-h") {
        print_help(runtime);
        std::process::exit(0);
    }

    let mut config = ServerConfig::default();
    if default_config_path.exists() {
        config.apply_file(default_config_path)?;
    }
    apply_config_args(&mut config, &args, runtime)?;
    let cli_overrides = has_cli_overrides(&args);
    let mut args = args.into_iter();

    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--help" | "-h" => {
                print_help(runtime);
                std::process::exit(0);
            }
            "--bind" => {
                config.bind = parse_value(&arg, args.next())?;
            }
            "--config" => {
                let _ = args
                    .next()
                    .ok_or_else(|| format!("missing value for {arg}"))?;
            }
            "--server-threads" => {
                config.server_threads = parse_value(&arg, args.next())?;
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
            "--low-memory" => {
                config.low_memory = true;
            }
            "--stats" => {
                config.stats_enabled = true;
            }
            "--tls-self-signed" => {
                config.tls_self_signed = true;
            }
            other => {
                return Err(format!("unknown argument: {other}").into());
            }
        }
    }

    if config.server_threads == 0 {
        return Err("--server-threads must be greater than zero".into());
    }

    if cli_overrides {
        config.config_sources.push("cli args".to_string());
    }

    Ok(config)
}

pub fn print_config(config: &ServerConfig, runtime: &str, actual_server_threads: usize) {
    println!("nacelle-stress-server effective config:");
    println!("  runtime: {runtime}");
    println!("  config_sources: {}", config.config_sources.join(" -> "));
    println!("  bind: {}", config.bind);
    println!("  server_threads: {}", config.server_threads);
    println!("  actual_server_threads: {actual_server_threads}");
    println!("  response_bytes: {}", config.response_bytes);
    println!("  read_buffer_capacity: {}", config.read_buffer_capacity);
    println!(
        "  response_buffer_capacity: {}",
        config.response_buffer_capacity
    );
    println!(
        "  request_body_chunk_size: {}",
        config.request_body_chunk_size
    );
    println!(
        "  request_body_channel_capacity: {}",
        config.request_body_channel_capacity
    );
    println!("  low_memory: {}", config.low_memory);
    println!("  stats_enabled: {}", config.stats_enabled);
    println!("  tls_self_signed: {}", config.tls_self_signed);
    println!("  limits:");
    println!("    max_connections: {}", config.limits.max_connections);
    println!(
        "    max_connections_per_peer: {}",
        config
            .limits
            .max_connections_per_peer
            .map(|max| max.to_string())
            .unwrap_or_else(|| "null".to_string())
    );
    println!(
        "    max_connection_opens_per_peer_per_second: {}",
        config
            .limits
            .max_connection_opens_per_peer_per_second
            .map(|max| max.to_string())
            .unwrap_or_else(|| "null".to_string())
    );
    println!(
        "    max_in_flight_requests: {}",
        config.limits.max_in_flight_requests
    );
    println!(
        "    max_streaming_tasks: {}",
        config.limits.max_streaming_tasks
    );
    println!("    max_memory_bytes: {}", config.limits.max_memory_bytes);
    println!(
        "    max_request_body_bytes: {}",
        config.limits.max_request_body_bytes
    );
    println!(
        "    max_response_body_bytes: {}",
        config.limits.max_response_body_bytes
    );
    println!(
        "    read_timeout_ms: {}",
        format_duration_ms(config.limits.read_timeout)
    );
    println!(
        "    write_timeout_ms: {}",
        format_duration_ms(config.limits.write_timeout)
    );
    println!(
        "    handler_timeout_ms: {}",
        format_duration_ms(config.limits.handler_timeout)
    );
    println!(
        "    idle_timeout_ms: {}",
        format_duration_ms(config.limits.idle_timeout)
    );
    println!(
        "    http_header_read_timeout_ms: {}",
        format_duration_ms(config.limits.http_header_read_timeout)
    );
    println!(
        "    http_request_body_read_timeout_ms: {}",
        format_duration_ms(config.limits.http_request_body_read_timeout)
    );
    println!(
        "    http_response_write_timeout_ms: {}",
        format_duration_ms(config.limits.http_response_write_timeout)
    );
    println!("    http_keep_alive: {}", config.limits.http_keep_alive);
    println!(
        "    http_max_connection_age_ms: {}",
        format_duration_ms(config.limits.http_max_connection_age)
    );
}

fn has_cli_overrides(args: &[String]) -> bool {
    let mut index = 0;
    while index < args.len() {
        match args[index].as_str() {
            "--config" => index += 2,
            "--help" | "-h" => index += 1,
            _ => return true,
        }
    }
    false
}

fn format_duration_ms(duration: Option<Duration>) -> String {
    match duration {
        Some(duration) => duration.as_millis().to_string(),
        None => "null".to_string(),
    }
}

fn apply_config_args(
    config: &mut ServerConfig,
    args: &[String],
    runtime: &str,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let mut index = 0;
    while index < args.len() {
        match args[index].as_str() {
            "--help" | "-h" => {
                print_help(runtime);
                std::process::exit(0);
            }
            "--config" => {
                let path = args
                    .get(index + 1)
                    .ok_or_else(|| "missing value for --config".to_string())?;
                config.apply_file(path)?;
                index += 2;
            }
            _ => {
                index += 1;
            }
        }
    }
    Ok(())
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

pub fn print_help(runtime: &str) {
    println!(
        "Nacelle stress server ({runtime})\n\
         \n\
         Runs a standalone nacelle server that can be targeted by an external\n\
         load generator (e.g. nacelle-stress-test with --bind pointing here).\n\
         \n\
         Usage:\n\
           cargo run --release --bin stress-{runtime} -- [options]\n\
         \n\
         Options:\n\
           --bind <addr>                             Listen address (default 127.0.0.1:7878)\n\
           --config <path>                           Load TOML config before applying CLI flags\n\
           --server-threads <count>                  Threads (default: logical CPUs)\n\
           --response-bytes <bytes>                  Response payload bytes per request (default 64)\n\
           --read-buffer <bytes>                     Read buffer capacity (default 65536)\n\
           --response-buffer <bytes>                 Response encode buffer capacity (default 16384)\n\
           --request-body-chunk-size <bytes>         Request body chunk size (default 16384)\n\
           --request-body-channel-capacity <count>   Request body channel capacity (default 8)\n\
           --low-memory                              Configure mimalloc to return freed pages to the\n\
                                                     OS immediately (purge_delay=0, no eager arena\n\
                                                     commit).  Reduces RSS on constrained systems at\n\
                                                     the cost of slightly higher allocation latency.\n\
                                                     Equivalent env vars (no recompile required):\n\
                                                       MIMALLOC_PURGE_DELAY=0\n\
                                                       MIMALLOC_ARENA_EAGER_COMMIT=0\n\
           --stats                                   Enable server-side per-request atomic counters.\n\
                                                     Leave disabled for peak throughput benchmarks.\n\
           --tls-self-signed                         Serve TCP over TLS with an ephemeral\n\
                                                     self-signed certificate. The stress server\n\
                                                     default build includes this capability, but\n\
                                                     plain TCP remains the runtime default.\n\
         \n\
         Config:\n\
           If ./config.toml exists, it is loaded automatically. Explicit\n\
           --config files are applied after ./config.toml, and CLI flags are\n\
           applied last.\n\
        "
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn toml_config_applies_limits() {
        let toml = r#"
bind = "127.0.0.1:9000"
server_threads = 4
response_bytes = 256
read_buffer_capacity = 4096
response_buffer_capacity = 2048
request_body_chunk_size = 1024
request_body_channel_capacity = 2
low_memory = true
stats_enabled = true
tls_self_signed = true

[limits]
max_connections = 128000
max_connections_per_peer = 4096
max_connection_opens_per_peer_per_second = 2048
max_in_flight_requests = 64000
max_streaming_tasks = 8192
max_memory_bytes = 8589934592
max_request_body_bytes = 16777216
max_response_body_bytes = 15728640
read_timeout_ms = 30000
write_timeout_ms = 30000
handler_timeout_ms = 60000
idle_timeout_ms = 120000
http_header_read_timeout_ms = 5000
http_request_body_read_timeout_ms = 10000
http_response_write_timeout_ms = 15000
http_keep_alive = false
http_max_connection_age_ms = 300000
"#;
        let file = toml::from_str::<ServerConfigFile>(toml).unwrap();
        let mut config = ServerConfig::default();
        config.apply_config_file(file);

        assert_eq!(config.bind, "127.0.0.1:9000".parse().unwrap());
        assert_eq!(config.server_threads, 4);
        assert_eq!(config.response_bytes, 256);
        assert_eq!(config.read_buffer_capacity, 4096);
        assert_eq!(config.response_buffer_capacity, 2048);
        assert_eq!(config.request_body_chunk_size, 1024);
        assert_eq!(config.request_body_channel_capacity, 2);
        assert!(config.low_memory);
        assert!(config.stats_enabled);
        assert!(config.tls_self_signed);
        assert_eq!(config.limits.max_connections, 128_000);
        assert_eq!(config.limits.max_connections_per_peer, Some(4_096));
        assert_eq!(
            config.limits.max_connection_opens_per_peer_per_second,
            Some(2_048)
        );
        assert_eq!(config.limits.max_in_flight_requests, 64_000);
        assert_eq!(config.limits.max_streaming_tasks, 8_192);
        assert_eq!(config.limits.max_memory_bytes, 8_589_934_592);
        assert_eq!(config.limits.max_request_body_bytes, 16_777_216);
        assert_eq!(config.limits.max_response_body_bytes, 15_728_640);
        assert_eq!(config.limits.read_timeout, Some(Duration::from_secs(30)));
        assert_eq!(config.limits.write_timeout, Some(Duration::from_secs(30)));
        assert_eq!(config.limits.handler_timeout, Some(Duration::from_secs(60)));
        assert_eq!(config.limits.idle_timeout, Some(Duration::from_secs(120)));
        assert_eq!(
            config.limits.http_header_read_timeout,
            Some(Duration::from_secs(5))
        );
        assert_eq!(
            config.limits.http_request_body_read_timeout,
            Some(Duration::from_secs(10))
        );
        assert_eq!(
            config.limits.http_response_write_timeout,
            Some(Duration::from_secs(15))
        );
        assert!(!config.limits.http_keep_alive);
        assert_eq!(
            config.limits.http_max_connection_age,
            Some(Duration::from_secs(300))
        );
    }

    #[test]
    fn cli_overrides_toml_after_config_load() {
        let path = std::env::temp_dir().join(format!(
            "nacelle-stress-server-test-{}.toml",
            std::process::id()
        ));
        std::fs::write(
            &path,
            "server_threads = 2\nresponse_bytes = 128\n[limits]\nmax_connections = 1500\n",
        )
        .unwrap();

        let config = parse_args(
            [
                "--config".to_string(),
                path.to_string_lossy().into_owned(),
                "--server-threads".to_string(),
                "8".to_string(),
            ],
            "tokio",
        )
        .unwrap();

        let _ = std::fs::remove_file(&path);
        assert_eq!(config.server_threads, 8);
        assert_eq!(config.response_bytes, 128);
        assert_eq!(config.limits.max_connections, 1_500);
    }

    #[test]
    fn cli_enables_self_signed_tls() {
        let config = parse_args(["--tls-self-signed".to_string()], "tokio").unwrap();
        assert!(config.tls_self_signed);
        assert_eq!(config.config_sources.last().unwrap(), "cli args");
    }

    #[test]
    fn missing_toml_values_keep_code_defaults() {
        let file =
            toml::from_str::<ServerConfigFile>("[limits]\nmax_connections = 1500\n").unwrap();
        let mut config = ServerConfig::default();
        let default_response_bytes = config.response_bytes;
        let default_response_body_bytes = config.limits.max_response_body_bytes;
        let default_handler_timeout = config.limits.handler_timeout;

        config.apply_config_file(file);

        assert_eq!(config.response_bytes, default_response_bytes);
        assert_eq!(config.limits.max_connections, 1_500);
        assert_eq!(
            config.limits.max_response_body_bytes,
            default_response_body_bytes
        );
        assert_eq!(config.limits.handler_timeout, default_handler_timeout);
    }

    #[test]
    fn default_config_toml_loads_when_present() {
        let path = std::env::temp_dir().join(format!(
            "nacelle-stress-server-default-config-{}.toml",
            std::process::id()
        ));
        std::fs::write(
            &path,
            "response_bytes = 512\n[limits]\nmax_connections = 1500\n",
        )
        .unwrap();

        let config = parse_args_with_default_config([], "tokio", &path).unwrap();

        let _ = std::fs::remove_file(&path);
        assert_eq!(config.response_bytes, 512);
        assert_eq!(config.limits.max_connections, 1_500);
        assert_eq!(
            config.config_sources,
            vec![
                "code defaults".to_string(),
                format!("toml {}", path.display())
            ]
        );
    }

    #[test]
    fn explicit_config_and_cli_override_default_config_toml() {
        let base_path = std::env::temp_dir().join(format!(
            "nacelle-stress-server-base-config-{}.toml",
            std::process::id()
        ));
        let explicit_path = std::env::temp_dir().join(format!(
            "nacelle-stress-server-explicit-config-{}.toml",
            std::process::id()
        ));
        std::fs::write(
            &base_path,
            "response_bytes = 128\n[limits]\nmax_connections = 1500\n",
        )
        .unwrap();
        std::fs::write(
            &explicit_path,
            "response_bytes = 256\n[limits]\nmax_connections = 32000\n",
        )
        .unwrap();

        let config = parse_args_with_default_config(
            [
                "--config".to_string(),
                explicit_path.to_string_lossy().into_owned(),
                "--response-bytes".to_string(),
                "1024".to_string(),
            ],
            "tokio",
            &base_path,
        )
        .unwrap();

        let _ = std::fs::remove_file(base_path);
        let _ = std::fs::remove_file(explicit_path);
        assert_eq!(config.response_bytes, 1024);
        assert_eq!(config.limits.max_connections, 32_000);
        assert_eq!(config.config_sources.last().unwrap(), "cli args");
    }

    #[test]
    fn explicit_config_without_cli_override_does_not_record_cli_source() {
        let path = std::env::temp_dir().join(format!(
            "nacelle-stress-server-explicit-only-config-{}.toml",
            std::process::id()
        ));
        std::fs::write(&path, "response_bytes = 256\n").unwrap();

        let config = parse_args_with_default_config(
            ["--config".to_string(), path.to_string_lossy().into_owned()],
            "tokio",
            Path::new("nacelle-stress-server-test-missing-default-config.toml"),
        )
        .unwrap();

        let _ = std::fs::remove_file(&path);
        assert_eq!(config.response_bytes, 256);
        assert_eq!(
            config.config_sources,
            vec![
                "code defaults".to_string(),
                format!("toml {}", path.display())
            ]
        );
    }
}
