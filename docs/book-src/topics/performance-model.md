# Performance model

nacelle's high-throughput raw TCP path is sensitive to small per-request costs.
When comparing runs, keep these variables fixed:

- commit
- Linux kernel and CPU governor
- allocator configuration
- server threads
- connection count
- pipeline depth
- payload size
- TLS versus plain TCP
- stress client version

Use the performance how-to for repeatable command lines.

{{#include ../../performance-tuning.md}}

