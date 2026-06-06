# Architecture

Nacelle is organized around a small transport/runtime core.

## Request Flow

```text
listener
  -> connection limit
  -> connection task
  -> protocol/HTTP decode
  -> request limit
  -> handler
  -> response body encode/stream
```

Raw TCP uses a `Protocol<Req>` trait to decode request heads and encode response
frames. HTTP uses Hyper HTTP/1 and maps requests into the same
`NacelleRequest` / `NacelleResponse` shape.

## Runtime State

`NacelleRuntimeState` owns shared budgets and counters. Connection, request, and
streaming-task limits are non-blocking atomic bounded counters. Memory uses a
checked reservation object that releases on drop.

This keeps the common request path allocation-light while still enforcing
bounded defaults.

## Bodies

`NacelleBody` has three internal shapes:

- empty/single chunk for fast small responses
- buffered chunks for decoded raw TCP bodies already in memory
- streaming channel for request/response bodies that move asynchronously

Raw TCP large request bodies reserve their declared length while streaming. HTTP
request bodies reserve `Content-Length` when Hyper exposes a bounded size hint.

## Shutdown

Listeners own a `JoinSet` of accepted connection tasks. Shutdown proceeds in
stages:

1. signal shutdown
2. stop accepting
3. drain active connection tasks
4. abort remaining tasks after the drain deadline
5. emit shutdown telemetry

Task tracking is at the connection boundary, not the per-request hot path.

## Observability

Telemetry is deliberately low-cardinality. Reasons are static strings such as
`connections`, `request_body_bytes`, or `http_body_read`.

With `otel`, active gauges are observable instruments backed by runtime-state
atomics, so collection reads current values without per-request metric writes.
