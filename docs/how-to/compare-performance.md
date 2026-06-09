# Compare performance profiles

Use separate profiles for each transport mode:

- plain TCP
- TCP with low-memory allocator behavior
- TCP with TLS
- HTTP

Do not compare TLS and non-TLS runs as if they measure the same path. Likewise,
do not compare two runs if the stress client version changed.

Recommended plain TCP baseline config:

```text
examples/nacelle-stress-server/configs/tcp.toml
```

Then run:

```bash
./build-all.sh
./run-tokio.sh --config examples/nacelle-stress-server/configs/tcp.toml --connections 256 --pipeline 8 --duration-secs 30 --payload-bytes 256
```

More background:

The current `main` branch has been observed around 1.9M RPS on Linux for the TCP benchmark path. This branch should be compared against that baseline on the same host, kernel, CPU governor, allocator settings, and command line.

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
./run-tokio.sh --config examples/nacelle-stress-server/configs/tcp.toml --server-threads 48 --connections 256 --pipeline 8 --duration-secs 30 --payload-bytes 256
```

The stress server defaults `stats_enabled = false` to avoid contended global
per-request atomics in peak throughput runs. Add `--stats` or set
`stats_enabled = true` when validating server-side counters rather than maximum
RPS.

The checked-in root `config.toml` enables self-signed TCP TLS for local
stress runs. For the plain TCP throughput baseline, use
`examples/nacelle-stress-server/configs/tcp.toml`. Compare TLS and non-TLS runs
separately:

```bash
./run-tokio.sh --config examples/nacelle-stress-server/configs/tcp.toml
./run-tokio.sh --config examples/nacelle-stress-server/configs/tcp-low-memory.toml
./run-tokio.sh --config examples/nacelle-stress-server/configs/tcp-tls.toml
```

The `run-tokio.sh` and `run-tokio.ps1` helpers apply root `config.toml` first,
then the selected profile, and choose the matching client mode automatically.

Guardrails:

- keep shutdown task tracking at the connection/listener boundary
- avoid per-request locks in the TCP hot path
- keep telemetry sinks optional; default operation should not push into in-memory sinks
- preserve single-chunk body fast paths
- tune TCP buffer sizes for the connection count instead of relying on large defaults
