# HTTP Hardening

Nacelle's HTTP transport is Hyper HTTP/1. Configure HTTP timeout and body policy through `NacelleLimits`, and configure request-shape policy through `NacelleHttpPolicy`.

Defaults:

- `http_header_read_timeout`: 30 seconds, enforced with Hyper's HTTP/1 header timeout and `TokioTimer`.
- `http_request_body_read_timeout`: 30 seconds, enforced while reading body frames.
- `http_response_write_timeout`: 30 seconds, enforced at Hyper's I/O write boundary.
- `http_keep_alive`: enabled.
- `http_max_connection_age`: disabled by default.
- request and response body size limits: 16 MiB each.

`NacelleHttpPolicy` can reject requests before the handler runs:

- allowed Host headers
- allowed HTTP methods
- maximum URI length
- maximum header count
- maximum aggregate header bytes
- optional per-peer connection caps through `NacelleLimits::with_max_connections_per_peer`

Rejected requests receive deterministic HTTP responses where the request parser has already accepted the request: `405`, `414`, `421`, or `431`. Rejections emit low-cardinality telemetry reasons such as `host`, `method_not_allowed`, `uri_too_long`, `header_count`, and `header_bytes`.

Enable the `tls` feature to terminate HTTP over Rustls. `NacelleTlsConfig` loads PEM certificate/key pairs, accepts explicit Rustls `ServerConfig` values, and enforces a TLS handshake timeout. Enable `tls-self-signed` only when local load tests or auto-deploying applications need to generate a self-signed certificate immediately.

For internet-facing deployments, a reverse proxy or load balancer can still own coarse traffic filtering and certificate automation. Nacelle now also enforces application-level body, request, connection, per-peer connection, timeout, TLS handshake, and optional Host/header/method/URI limits in-process.

Slowloris-style clients are closed by `http_header_read_timeout`. Trickle request bodies are closed by `http_request_body_read_timeout`. Slow response readers are closed by `http_response_write_timeout` when socket writes stop making progress.
