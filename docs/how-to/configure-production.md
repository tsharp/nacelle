# Configure production limits

Start from `NacelleLimits::default()` and tune downward for smaller machines. Avoid `usize::MAX` limits outside benchmarks.

Recommended presets:

- Internal service: keep defaults, set body limits to the largest expected payload, and run behind process supervision.
- Internet-facing behind proxy: cap connections and requests to the container budget, keep 30 second header/body/write timeouts, and let the proxy own coarse traffic filtering or certificate automation when desired.
- Proxy-aware HTTP: configure `NacelleHttpPolicy::with_trusted_proxy_ips(...)` only with known proxy addresses before allowing `Forwarded` or `X-Forwarded-For` to affect per-peer request limits or request metadata.
- Direct HTTPS listener: enable `http,tls`, load certificate/key material through `NacelleTlsConfig`, configure an SNI allowlist with `from_pem_with_allowed_server_names` or `from_der_with_allowed_server_names`, set a short TLS handshake timeout, configure `max_connections_per_peer` and `max_connection_opens_per_peer_per_second`, enable HTTP access logs, and attach `NacelleHttpPolicy` with Host, method, URI, header, security-header, and per-peer request-rate limits.
- Direct TCP TLS listener: enable `tcp,tls` with `NacelleTlsConfig` or `tcp,openssl` with `NacelleOpenSslConfig`, use `NacelleProtocols::tcp_openssl(...)` for required OpenSSL in the app host path, and keep protocol-level authentication/authorization in the application protocol.
- Optional TCP OpenSSL listener: enable `tcp,openssl` and use `serve_tcp_optional_openssl(...)` or the matching host/app builder method when one listener must accept both plain and TLS clients; keep `NacelleTlsDetectionOptions::timeout` short enough to avoid tying up idle accepted connections.
- IPv4 plus IPv6 TCP bind: use the `NacelleProtocols::*_dual_stack(...)` helpers when a serve-based app should bind both wildcard families for one protocol. The helpers register separate IPv4 and IPv6 listeners and force the IPv6 listener to v6-only mode.
- Unix socket listener: enable `tcp` on Unix and call `serve_unix(...)` or `NacelleHost::enable_unix_socket(...)`; use `NacelleUnixSocketOptions` only when this process owns stale-path cleanup or socket-file permissions.
- Local load-test/autodeploy HTTPS: enable `tls-self-signed` and call `NacelleTlsConfig::self_signed(...)`; do not treat generated certificates as a public trust or rotation strategy.
- High concurrency: reduce TCP buffer capacities before raising `max_connections`.

Memory budget:

```text
connection_budget =
  max_connections * (read_buffer_capacity + response_buffer_capacity)
body_budget =
  concurrent_buffered_or_streaming_bodies * max_request_body_bytes
total_budget =
  connection_budget + body_budget + handler/backend/runtime headroom
```

TCP processes requests sequentially per connection. `request_body_channel_capacity` controls the queued streaming chunks between the socket reader and handler. HTTP uses Hyper's internal buffers plus Nacelle's body queue, so leave extra headroom when enabling large request bodies.

Use `NacelleTcpOptions` for accepted TCP stream behavior. Defaults preserve the
existing behavior: `TCP_NODELAY` enabled and TCP keepalive disabled. Enable
keepalive deliberately per deployment target because OS defaults and supported
fields vary. `NacelleTcpBindOptions` adds listener bind controls such as
IPv6-only mode for APIs that need explicit family behavior.

Dangerous configurations:

- unbounded connections with large per-connection buffers
- large body limits without a process/container memory limit
- disabled timeouts on internet-facing listeners
- direct internet-facing HTTP without Host/header/method/URI policy
- direct internet-facing TLS without an SNI allowlist
- direct internet-facing listeners without per-peer connection caps
- direct internet-facing listeners without per-peer connection-open rate caps
- direct internet-facing HTTP without per-peer request caps and access logs
- trusting forwarded peer headers without an explicit trusted proxy list
- generated self-signed certificates used as a long-lived public-edge certificate strategy
- high keep-alive connection counts without proxy-level idle limits
- long TLS detection timeouts on optional TLS listeners
- Unix stale-path cleanup for a socket path not exclusively owned by this process

TLS certificate rotation:

```rust
let tls = NacelleTlsConfig::from_pem_files("cert.pem", "key.pem")?;
tls.reload_from_pem_files("next-cert.pem", "next-key.pem")?;
```

Reloads affect new TLS handshakes. Existing connections continue with the
configuration negotiated when they connected.
