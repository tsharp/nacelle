# Nacelle Documentation

Nacelle is an experimental Tokio-based Rust library for streaming application
handlers across raw TCP and HTTP transports.

Use this documentation for application integration, operational limits,
transport behavior, and production validation.

## Start Here

- [Usage guide](usage.md) shows the shared handler shape, raw TCP setup, HTTP
  setup, multi-listener hosts, limits, shutdown, and telemetry.
- [Architecture](architecture.md) explains the request flow, runtime state,
  body model, shutdown lifecycle, and observability strategy.
- [Operations](operations.md) covers production runtime behavior and operator
  expectations.

## Production

- [HTTP hardening](http-hardening.md)
- [Production configuration](production-configuration.md)
- [Stress testing](stress-testing.md)
- [Security scanning](security-scanning.md)
- [Performance tuning](performance-tuning.md)
- [API stability](api-stability.md)

Internal readiness plans and assessments live under `docs/internal` and are not
included in the generated DocFX site.
