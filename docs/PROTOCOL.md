# Nacelle Length-Delimited Protocol

This document describes the built-in `LengthDelimitedProtocol` used by the
prototype server. Custom protocols can implement `Protocol<Req>` directly.

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
`RequestBody`. Small bodies are served from the connection read buffer. Larger
bodies are streamed to the handler in configured chunks.

`opcode` is request metadata. The application handler decides whether to use it
for routing, reject it, or ignore it. If the handler rejects an opcode after
draining the body and returns an error, the server encodes that error as a
response frame.

## Responses

Handlers write zero or more body chunks through `ResponseWriter`.

The protocol guarantees:

- the first response frame has `FRAME_FLAG_START`
- the last response frame has `FRAME_FLAG_END`
- a handler that writes no body still emits a start/end response frame
- a handler error emits a start/end/error frame when the handler has not already
  started writing a response

Responses are written in request-processing order for a single connection. The
prototype does not yet provide concurrent per-connection response interleaving.

## Error Handling

Malformed frame heads, oversized frames, and EOF before a complete frame cause
the connection to fail. Unknown opcodes and handler errors are encoded as error
frames when enough request context is available.

## Limits

The server enforces `NacelleConfig::max_frame_len` against `frame_len`.
Buffer sizes and request-body chunking are configured through `NacelleConfig`.
