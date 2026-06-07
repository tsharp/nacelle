# Compare performance profiles

Use separate profiles for each transport mode:

- plain raw TCP
- raw TCP with low-memory allocator behavior
- raw TCP with TLS
- HTTP

Do not compare TLS and non-TLS runs as if they measure the same path. Likewise,
do not compare two runs if the stress client version changed.

Recommended plain raw TCP baseline config:

```toml
low_memory = true
tls_self_signed = false

[limits]
max_memory_bytes = 536870912
```

Then run:

```bash
./build-all.sh
./run-tokio.sh --connections 256 --pipeline 8 --duration-secs 30 --payload-bytes 256
```

More background:

{{#include ../../performance-tuning.md}}

