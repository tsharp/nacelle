# API Stability

Nacelle is `0.1.x`, so public APIs are still experimental.

Stable enough for prototype integrations:

- `NacelleRequest`, `NacelleResponse`, and `NacelleBody`
- `Handler` and `handler_fn`
- `NacelleLimits` and `NacelleRuntimeState`
- `NacelleHost`

Experimental:

- transport-specific metadata
- telemetry sink details
- stress tooling config
- feature combinations involving `tower` and `otel`

Before `1.0`, minor releases may change defaults or builder methods when production safety requires it. After `1.0`, public API changes should follow semver, with migration notes for config/default changes.
