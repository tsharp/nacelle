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
cargo run --release --package nacelle-stress-server --bin tokio-server -- --config nacelle-stress-server/config.example.toml
cargo run --release --package nacelle-stress-test -- --connections 128 --pipeline 64 --duration-secs 30
```

The stress server defaults `stats_enabled = false` to avoid contended global
per-request atomics in peak throughput runs. Add `--stats` or set
`stats_enabled = true` when validating server-side counters rather than maximum
RPS.

The checked-in root `config.toml` enables self-signed raw TCP TLS for local
stress runs. For the plain TCP throughput baseline, use an explicit config such
as `nacelle-stress-server/config.example.toml` with `tls_self_signed = false`
and omit `--tls-insecure` from the stress client. Compare TLS and non-TLS runs
separately.

The `run-tokio.sh` and `run-tokio.ps1` helpers read root `config.toml` and choose
the matching client mode automatically.

Guardrails:

- keep shutdown task tracking at the connection/listener boundary
- avoid per-request locks in the raw TCP hot path
- keep telemetry sinks optional; default operation should not push into in-memory sinks
- preserve single-chunk body fast paths
- tune raw TCP buffer sizes for the connection count instead of relying on large defaults
