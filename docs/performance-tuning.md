# Performance Tuning

The current `main` branch has been observed around 1.9M RPS on Linux for the raw TCP benchmark path. This branch should be compared against that baseline on the same host, kernel, CPU governor, allocator settings, and command line.

Suggested local benchmark:

```bash
cargo bench -p nacelle --features bench,reference_protocol
```

The `runtime_limits` benchmark group covers connection/request permit
acquire/drop and memory reservation overhead. Watch it closely after changes to
`NacelleRuntimeState`.

Suggested RPS comparison:

```bash
./build-all.sh
./run-tokio.sh --config nacelle-stress-server/configs/raw-tcp.toml --server-threads 48 --connections 256 --pipeline 8 --duration-secs 30 --payload-bytes 256
```

The stress server defaults `stats_enabled = false` to avoid contended global
per-request atomics in peak throughput runs. Add `--stats` or set
`stats_enabled = true` when validating server-side counters rather than maximum
RPS.

The checked-in root `config.toml` enables self-signed raw TCP TLS for local
stress runs. For the plain TCP throughput baseline, use
`nacelle-stress-server/configs/raw-tcp.toml`. Compare TLS and non-TLS runs
separately:

```bash
./run-tokio.sh --config nacelle-stress-server/configs/raw-tcp.toml
./run-tokio.sh --config nacelle-stress-server/configs/raw-tcp-low-memory.toml
./run-tokio.sh --config nacelle-stress-server/configs/raw-tcp-tls.toml
```

The `run-tokio.sh` and `run-tokio.ps1` helpers apply root `config.toml` first,
then the selected profile, and choose the matching client mode automatically.

Guardrails:

- keep shutdown task tracking at the connection/listener boundary
- avoid per-request locks in the raw TCP hot path
- keep telemetry sinks optional; default operation should not push into in-memory sinks
- preserve single-chunk body fast paths
- tune raw TCP buffer sizes for the connection count instead of relying on large defaults
