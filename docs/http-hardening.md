# HTTP Hardening

Nacelle's HTTP transport is Hyper HTTP/1. Configure HTTP-specific policy through `NacelleLimits`.

Defaults:

- `http_header_read_timeout`: 30 seconds, enforced with Hyper's HTTP/1 header timeout and `TokioTimer`.
- `http_request_body_read_timeout`: 30 seconds, enforced while reading body frames.
- `http_response_write_timeout`: 30 seconds, enforced at Hyper's I/O write boundary.
- `http_keep_alive`: enabled.
- `http_max_connection_age`: disabled by default.
- request and response body size limits: 16 MiB each.

For internet-facing deployments, prefer running behind a reverse proxy or load balancer that terminates TLS, applies coarse connection policy, and caps header size. Nacelle still enforces application-level body, request, connection, and timeout limits behind that proxy.

Slowloris-style clients are closed by `http_header_read_timeout`. Trickle request bodies are closed by `http_request_body_read_timeout`. Slow response readers are closed by `http_response_write_timeout` when socket writes stop making progress.
