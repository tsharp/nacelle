# Stress Testing

Run the server:

```bash
cargo run --release --package nacelle-stress-server --bin tokio-server -- --config nacelle-stress-server/config.example.toml
```

Run a bounded client smoke test:

```bash
cargo run --release --package nacelle-stress-test -- --tls-insecure --connections 32 --pipeline 16 --duration-secs 15
```

Server-side stats are disabled by default for peak throughput. Add `--stats`
when you want periodic server counters during a diagnostic run.

The Tokio stress server default build includes `tls-self-signed` support. The
checked-in root `config.toml` enables `tls_self_signed = true`, so the local
stress client should use `--tls-insecure` with that default config. Use an
explicit TOML file with `tls_self_signed = false` when you need a plain TCP
baseline.

CI-friendly scenarios should stay short and deterministic:

- baseline echo throughput
- max connection cap
- max request cap
- slow reader
- slow writer
- graceful shutdown under load

Heavy RPS and soak tests should run manually or nightly on dedicated Linux hosts.
