# Runtime limits and backpressure

Runtime limits are enforced through `NacelleRuntimeState`. They are intended to
make overload predictable rather than perfectly invisible.

Key budgets include:

- active connections
- in-flight requests
- streaming body tasks
- optional per-peer connections
- memory reservations
- request and response body size
- read, write, handler, idle, HTTP, and TLS handshake timeouts

The important production habit is to size limits together. A high connection
count with large read and response buffers is a memory budget decision, not just
a concurrency decision.

For configuration details:

{{#include ../../production-configuration.md}}

