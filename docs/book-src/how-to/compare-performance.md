# Compare performance profiles

Use separate profiles for each transport mode:

- plain raw TCP
- raw TCP with low-memory allocator behavior
- raw TCP with TLS
- HTTP

Do not compare TLS and non-TLS runs as if they measure the same path. Likewise,
do not compare two runs if the stress client version changed.

Recommended plain raw TCP baseline config:

```text
nacelle-stress-server/configs/raw-tcp.toml
```

Then run:

```bash
./build-all.sh
./run-tokio.sh --config nacelle-stress-server/configs/raw-tcp.toml --connections 256 --pipeline 8 --duration-secs 30 --payload-bytes 256
```

More background:

{{#include ../../performance-tuning.md}}
