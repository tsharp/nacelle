# Production Configuration

Start from `NacelleLimits::default()` and tune downward for smaller machines. Avoid `usize::MAX` limits outside benchmarks.

Recommended presets:

- Internal service: keep defaults, set body limits to the largest expected payload, and run behind process supervision.
- Internet-facing behind proxy: cap connections and requests to the container budget, keep 30 second header/body/write timeouts, and let the proxy own TLS and coarse traffic filtering.
- High concurrency: reduce raw TCP buffer capacities before raising `max_connections`.

Memory budget:

```text
connection_budget =
  max_connections * (read_buffer_capacity + response_buffer_capacity)
body_budget =
  concurrent_buffered_or_streaming_bodies * max_request_body_bytes
total_budget =
  connection_budget + body_budget + handler/backend/runtime headroom
```

Raw TCP processes requests sequentially per connection. `request_body_channel_capacity` controls the queued streaming chunks between the socket reader and handler. HTTP uses Hyper's internal buffers plus Nacelle's body queue, so leave extra headroom when enabling large request bodies.

Dangerous configurations:

- unbounded connections with large per-connection buffers
- large body limits without a process/container memory limit
- disabled timeouts on internet-facing listeners
- high keep-alive connection counts without proxy-level idle limits
