// Shared service/config logic used by all runtime-specific entry points.

use std::net::SocketAddr;
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
pub struct StressService {
    pub response_payload: Bytes,
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
        "
    );
}
