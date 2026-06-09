# Run the stress harness

The stress harness has two binaries:

- `tokio-server`, from `nacelle-stress-server`
- `nacelle-stress-test`, from `nacelle-stress-test`

Build both:

```bash
./build-all.sh
```

Then run the convenience script:

```bash
./run-tokio.sh
```

The script reads root `config.toml` by default. Pass `--config` to select a
repeatable benchmark profile. If the effective `tls_self_signed` value is true,
it passes `--tls-insecure` to the client so the server and client speak the
same transport.

For a plain TCP baseline, use
`nacelle-stress-server/configs/tcp.toml`.

For full details, see the how-to guide:

Run the server:

```bash
cargo run --release --package nacelle-stress-server --bin tokio-server -- --config nacelle-stress-server/configs/tcp.toml
```

Run a bounded client smoke test:

```bash
cargo run --release --package nacelle-stress-test -- --connections 32 --pipeline 16 --duration-secs 15
```

The `run-tokio.sh` and `run-tokio.ps1` helpers accept `--config`/`-Config` and
pass `--tls-insecure` to the stress client only when the effective
`tls_self_signed` value is true.

The stress client enables its Rustls support by default so `--tls-insecure`
works with the local self-signed server. For a Rustls-free plain TCP build, run
both stress binaries with `--no-default-features` and use
`nacelle-stress-server/configs/tcp.toml`.

Repeatable profiles:

- `nacelle-stress-server/configs/tcp.toml`: plain TCP baseline.
- `nacelle-stress-server/configs/tcp-low-memory.toml`: plain TCP with
  mimalloc low-memory behavior enabled.
- `nacelle-stress-server/configs/tcp-tls.toml`: TCP wrapped in
  self-signed TLS.

Linux example:

```bash
./build-all.sh
./run-tokio.sh --config nacelle-stress-server/configs/tcp.toml --server-threads 48 --connections 256 --pipeline 8 --duration-secs 30 --payload-bytes 256
```

PowerShell example:

```powershell
.\run-tokio.ps1 -Config nacelle-stress-server/configs/tcp.toml -ServerThreads 48 -Connections 256 -Pipeline 8 -DurationSecs 30 -PayloadBytes 256
```

Server-side stats are disabled by default for peak throughput. Add `--stats`
when you want periodic server counters during a diagnostic run.

The Tokio stress server default build includes `tls-self-signed` support. The
checked-in root `config.toml` enables `tls_self_signed = true`, so the local
stress client should use `--tls-insecure` with that default config. Use
`--no-default-features` with `nacelle-stress-server/configs/tcp.toml` when
you need a Rustls-free plain TCP baseline.

CI-friendly scenarios should stay short and deterministic:

- baseline echo throughput
- max connection cap
- max request cap
- slow reader
- slow writer
- graceful shutdown under load

Heavy RPS and soak tests should run manually or nightly on dedicated Linux hosts.
