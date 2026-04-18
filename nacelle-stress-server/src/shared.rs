// Shared service/config logic used by all runtime-specific entry points.

use std::net::SocketAddr;
use std::os::raw::c_long;
use std::str::FromStr;
use std::sync::Arc;

use bytes::Bytes;
use nacelle::{
    FrameRequest, LengthDelimitedProtocol, NacelleConfig, NacelleError, NacelleServer, RequestBody,
    ResponseWriter, handler_fn,
};

pub const STRESS_OPCODE: u64 = 1;

#[derive(Debug, Clone)]
pub struct ServerConfig {
    pub bind: SocketAddr,
    pub server_threads: usize,
    pub response_bytes: usize,
    pub max_concurrent_requests_per_connection: usize,
    pub read_buffer_capacity: usize,
    pub response_buffer_capacity: usize,
    pub request_body_chunk_size: usize,
    pub request_body_channel_capacity: usize,
    pub low_memory: bool,
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
            low_memory: false,
        }
    }
}

#[derive(Debug)]
pub struct StressService {
    pub response_payload: Bytes,
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

pub fn parse_args(
    args: impl IntoIterator<Item = String>,
    runtime: &str,
) -> Result<ServerConfig, Box<dyn std::error::Error + Send + Sync>> {
    let mut config = ServerConfig::default();
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
            "--low-memory" => {
                config.low_memory = true;
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
           --server-threads <count>                  Threads (default: logical CPUs)\n\
           --response-bytes <bytes>                  Response payload bytes per request (default 64)\n\
           --max-concurrent-requests-per-connection <count>\n\
                                                     Server-side overlap per connection (default 8)\n\
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
        "
    );
}
