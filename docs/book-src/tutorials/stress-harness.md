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

The script reads root `config.toml`. If `tls_self_signed = true`, it passes
`--tls-insecure` to the client so the server and client speak the same
transport.

For a plain raw TCP baseline, use a config with:

```toml
tls_self_signed = false
```

For full details, see the how-to guide:

{{#include ../../stress-testing.md}}

