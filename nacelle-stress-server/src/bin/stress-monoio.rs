// Monoio (io_uring / epoll thread-per-core) stress server.
//
// Each OS thread runs its own monoio runtime and accepts connections on the
// same port via SO_REUSEPORT (Linux).  nacelle's built-in `serve_tcp` drives
// the accept loop and hands each connection to `serve_halves`.
//
// Build and run:
//   cargo build --release -p nacelle-stress-server \
//       --no-default-features --features monoio-runtime
//   ./target/release/stress-monoio [options]

#[global_allocator]
static ALLOC: mimalloc::MiMalloc = mimalloc::MiMalloc;

#[path = "../shared.rs"]
mod shared;
use shared::{build_server, configure_allocator, parse_args};

fn main() {
    let config = parse_args(std::env::args().skip(1), "monoio").expect("invalid args");
    configure_allocator(config.low_memory);
    let server = build_server(&config).expect("failed to build server");
    let addr = config.bind;
    let n_threads = config.server_threads.max(1);

    println!("nacelle-stress-server (monoio) listening on {addr} (threads={n_threads})");

    let threads: Vec<_> = (0..n_threads)
        .map(|i| {
            let server = server.clone();
            std::thread::Builder::new()
                .name(format!("monoio-{i}"))
                .spawn(move || {
                    monoio::RuntimeBuilder::<monoio::FusionDriver>::new()
                        .build()
                        .expect("failed to build monoio runtime")
                        .block_on(async move {
                            server.serve_tcp(addr).await.expect("serve_tcp failed");
                        });
                })
                .expect("failed to spawn thread")
        })
        .collect();

    for t in threads {
        t.join().ok();
    }
}
