# Reference protocol

This document describes the optional `LengthDelimitedProtocol` reference
implementation enabled by the `reference_protocol` feature. Custom raw TCP
protocols can implement `Protocol<Req>` directly.

## Frame Layout

All integer fields are little-endian.

| Offset | Size | Field |
| --- | ---: | --- |
| 0 | 4 | `frame_len` |
| 4 | 8 | `request_id` |
| 12 | 8 | `opcode` |
| 20 | 4 | `flags` |
| 24 | `frame_len - 20` | body bytes |

`frame_len` counts the fixed fields after itself plus the body. The minimum
valid value is `20`.

## Flags

| Flag | Value | Meaning |
| --- | ---: | --- |
| `FRAME_FLAG_START` | `0b0001` | First response frame for a request |
| `FRAME_FLAG_END` | `0b0010` | Last response frame for a request |
| `FRAME_FLAG_ERROR` | `0b0100` | Response body contains an error message |

Request flags are decoded and preserved in `FrameRequest`, but the built-in
server does not currently interpret request flags.

## Requests

Each request frame contains one complete request body. The server decodes only
the frame head before dispatch, then exposes the body to the handler as a
`NacelleBody`. Small bodies are served from the connection read buffer. Larger
bodies are streamed to the handler in configured chunks.

`opcode` is request metadata. The application handler decides whether to use it
for routing, reject it, or ignore it. If the handler rejects an opcode after
draining the body and returns an error, the server encodes that error as a
response frame.

## Responses

Handlers return a `NacelleResponse` with a streaming `NacelleBody`. The raw TCP
transport encodes that response body into one or more response frames.
By default, raw TCP responses inherit `request_id` and `opcode` from the request
context. Applications can override either field with `RawTcpResponseMeta`.

The protocol guarantees:

- the first response frame has `FRAME_FLAG_START`
- the last response frame has `FRAME_FLAG_END`
- a handler that returns an empty body still emits a start/end response frame
- a handler error emits a start/end/error frame

Responses are written in request-processing order for a single connection. The
prototype does not yet provide concurrent per-connection response interleaving.

## Error Handling

Malformed frame heads, oversized frames, and EOF before a complete frame cause
the connection to fail. Handler errors are encoded as error frames when enough
request context is available. Unknown opcode handling is application policy.

## Limits

The server enforces `NacelleConfig::max_frame_len` against `frame_len`.
Buffer sizes and request-body chunking are configured through `NacelleConfig`.
Runtime budgets, timeouts, and active counters are configured through
`NacelleLimits` / `NacelleRuntimeState`.

Raw TCP request handling is sequential per connection. Pipelined frames can sit
in the socket/read buffer, but Nacelle does not run multiple handlers
concurrently for one raw TCP connection. Streaming request bodies use
`request_body_channel_capacity` for backpressure between socket reads and the
handler, and declared streaming body bytes are reserved against the memory
budget until the streaming request finishes.


