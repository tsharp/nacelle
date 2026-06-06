# Production Checklist

- Limits are explicitly configured for the service SKU.
- Graceful shutdown is wired through `NacelleHost::shutdown_and_wait_timeout`.
- Metrics/tracing are exported and include rejection, timeout, and shutdown events.
- Active gauges are visible in dashboards.
- HTTP request and response byte metrics are visible when HTTP is enabled.
- Load balancer/proxy timeout and body-size policy is known.
- HTTP header, body, response write, keep-alive, and max-age policy is documented.
- Direct public edge deployments satisfy the separate [edge readiness plan](edge-readiness-plan-2026-06-06.md).
- Raw TCP frame size and sequential per-connection processing are documented for clients.
- Stress smoke tests have run with expected concurrency and duration.
- `scripts/validate-production-readiness.sh` or `scripts/validate-production-readiness.ps1` passes.
- Vulnerability scan is clean or advisories are explicitly accepted.
- Rollback strategy and previous known-good version are documented.
