# Performance Tuning

The current `main` branch has been observed around 1.9M RPS on Linux for the raw TCP benchmark path. This branch should be compared against that baseline on the same host, kernel, CPU governor, allocator settings, and command line.

Suggested local benchmark:

```bash
cargo bench -p nacelle --features bench,reference_protocol
```

Suggested RPS comparison:

```bash
cargo run --release --package nacelle-stress-server --bin tokio-server -- --config nacelle-stress-server/config.example.toml
cargo run --release --package nacelle-stress-test -- --connections 128 --pipeline 64 --duration-secs 30
```

Guardrails:

- keep shutdown task tracking at the connection/listener boundary
- avoid per-request locks in the raw TCP hot path
- keep telemetry sinks optional; default operation should not push into in-memory sinks
- preserve single-chunk body fast paths
- tune raw TCP buffer sizes for the connection count instead of relying on large defaults
