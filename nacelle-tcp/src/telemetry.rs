use std::sync::Arc;
use std::time::Duration;

use nacelle_core::error::NacelleError;
use nacelle_core::telemetry::NacelleTransport;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct NacelleTcpTelemetryConfig {
    pub metrics: bool,
    pub opcode_labels: bool,
}

impl Default for NacelleTcpTelemetryConfig {
    fn default() -> Self {
        Self {
            metrics: true,
            opcode_labels: false,
        }
    }
}

#[derive(Debug, Clone)]
pub struct NacelleTcpMetricsContext {
    pub transport: NacelleTransport,
    pub listener: Arc<str>,
    pub protocol: &'static str,
    pub tls: &'static str,
    pub opcode: Option<u64>,
}

impl NacelleTcpMetricsContext {
    pub fn new(
        transport: NacelleTransport,
        listener: Arc<str>,
        protocol: &'static str,
        tls: &'static str,
        opcode: Option<u64>,
    ) -> Self {
        Self {
            transport,
            listener,
            protocol,
            tls,
            opcode,
        }
    }
}

#[derive(Clone)]
pub struct NacelleTcpTelemetry {
    config: NacelleTcpTelemetryConfig,
    #[cfg(feature = "otel")]
    metrics: Arc<NacelleTcpOtelMetrics>,
}

impl std::fmt::Debug for NacelleTcpTelemetry {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("NacelleTcpTelemetry")
            .field("config", &self.config)
            .finish()
    }
}

impl Default for NacelleTcpTelemetry {
    fn default() -> Self {
        Self::new()
    }
}

impl NacelleTcpTelemetry {
    pub fn new() -> Self {
        Self {
            config: NacelleTcpTelemetryConfig::default(),
            #[cfg(feature = "otel")]
            metrics: Arc::new(NacelleTcpOtelMetrics::new()),
        }
    }

    pub fn with_config(mut self, config: NacelleTcpTelemetryConfig) -> Self {
        self.config = config;
        self
    }

    pub fn with_metrics(mut self, enabled: bool) -> Self {
        self.config.metrics = enabled;
        self
    }

    pub fn with_opcode_labels(mut self, enabled: bool) -> Self {
        self.config.opcode_labels = enabled;
        self
    }

    pub fn config(&self) -> NacelleTcpTelemetryConfig {
        self.config
    }

    pub fn metrics_enabled(&self) -> bool {
        cfg!(feature = "otel") && self.config.metrics
    }

    pub fn connection_accepted(&self, context: &NacelleTcpMetricsContext) {
        let _ = context;
        if !self.metrics_enabled() {
            return;
        }
        #[cfg(feature = "otel")]
        {
            let attributes = tcp_connection_attributes(context);
            self.metrics.connection_accepted.add(1, &attributes);
            self.metrics.connection_active.add(1, &attributes);
        }
    }

    pub fn connection_closed(
        &self,
        context: &NacelleTcpMetricsContext,
        close_reason: &'static str,
    ) {
        let _ = (context, close_reason);
        if !self.metrics_enabled() {
            return;
        }
        #[cfg(feature = "otel")]
        {
            let mut attributes = tcp_connection_attributes(context);
            attributes.push(opentelemetry::KeyValue::new("close_reason", close_reason));
            self.metrics.connection_closed.add(1, &attributes);
            let active_attributes = tcp_connection_attributes(context);
            self.metrics.connection_active.add(-1, &active_attributes);
        }
    }

    pub fn request_started(&self, context: &NacelleTcpMetricsContext) {
        let _ = context;
        if !self.metrics_enabled() {
            return;
        }
        #[cfg(feature = "otel")]
        {
            let attributes = tcp_request_attributes(context, self.config.opcode_labels);
            self.metrics.request_started.add(1, &attributes);
            self.metrics.request_in_flight.add(1, &attributes);
        }
    }

    pub fn request_completed(
        &self,
        context: &NacelleTcpMetricsContext,
        status: &'static str,
        request_bytes: usize,
        response_bytes: usize,
        elapsed: Duration,
    ) {
        let _ = (context, status, request_bytes, response_bytes, elapsed);
        if !self.metrics_enabled() {
            return;
        }
        #[cfg(feature = "otel")]
        {
            let mut attributes = tcp_request_attributes(context, self.config.opcode_labels);
            attributes.push(opentelemetry::KeyValue::new("status", status));
            self.metrics.request_completed.add(1, &attributes);
            self.metrics
                .request_duration_ms
                .record(elapsed.as_secs_f64() * 1_000.0, &attributes);
            if request_bytes != 0 {
                self.metrics
                    .request_bytes
                    .add(request_bytes as u64, &attributes);
            }
            if response_bytes != 0 {
                self.metrics
                    .response_bytes
                    .add(response_bytes as u64, &attributes);
            }

            let active_attributes = tcp_request_attributes(context, self.config.opcode_labels);
            self.metrics.request_in_flight.add(-1, &active_attributes);
        }
    }

    pub fn request_body_bytes(&self, context: &NacelleTcpMetricsContext, bytes: usize) {
        let _ = context;
        if bytes == 0 || !self.metrics_enabled() {
            return;
        }
        #[cfg(feature = "otel")]
        self.metrics.request_body_bytes.add(
            bytes as u64,
            &tcp_request_attributes(context, self.config.opcode_labels),
        );
    }

    pub fn phase_duration(
        &self,
        context: &NacelleTcpMetricsContext,
        phase: &'static str,
        elapsed: Duration,
    ) {
        let _ = (context, phase, elapsed);
        if !self.metrics_enabled() {
            return;
        }
        #[cfg(feature = "otel")]
        self.metrics.phase_duration_ms.record(
            elapsed.as_secs_f64() * 1_000.0,
            &tcp_phase_attributes(context, phase, self.config.opcode_labels),
        );
    }

    pub fn error(
        &self,
        context: &NacelleTcpMetricsContext,
        phase: &'static str,
        error: &NacelleError,
    ) {
        let _ = (context, phase, error);
        if !self.metrics_enabled() {
            return;
        }
        #[cfg(feature = "otel")]
        {
            self.metrics.errors.add(
                1,
                &tcp_error_attributes(context, phase, error, self.config.opcode_labels),
            );
            if let NacelleError::ResourceLimit(limit) = error {
                let mut attributes = tcp_request_attributes(context, self.config.opcode_labels);
                attributes.push(opentelemetry::KeyValue::new("limit", *limit));
                attributes.push(opentelemetry::KeyValue::new("phase", phase));
                self.metrics.resource_limit_rejections.add(1, &attributes);
            }
        }
    }
}

#[cfg(feature = "otel")]
fn tcp_connection_attributes(context: &NacelleTcpMetricsContext) -> Vec<opentelemetry::KeyValue> {
    vec![
        opentelemetry::KeyValue::new("listener", context.listener.as_ref().to_owned()),
        opentelemetry::KeyValue::new("transport", context.transport.as_str()),
        opentelemetry::KeyValue::new("tls", context.tls),
    ]
}

#[cfg(feature = "otel")]
fn tcp_request_attributes(
    context: &NacelleTcpMetricsContext,
    include_opcode: bool,
) -> Vec<opentelemetry::KeyValue> {
    let mut attributes = tcp_connection_attributes(context);
    attributes.push(opentelemetry::KeyValue::new("protocol", context.protocol));
    if include_opcode && let Some(opcode) = context.opcode {
        attributes.push(opentelemetry::KeyValue::new("opcode", opcode.to_string()));
    }
    attributes
}

#[cfg(feature = "otel")]
fn tcp_phase_attributes(
    context: &NacelleTcpMetricsContext,
    phase: &'static str,
    include_opcode: bool,
) -> Vec<opentelemetry::KeyValue> {
    let mut attributes = tcp_request_attributes(context, include_opcode);
    attributes.push(opentelemetry::KeyValue::new("phase", phase));
    attributes
}

#[cfg(feature = "otel")]
fn tcp_error_attributes(
    context: &NacelleTcpMetricsContext,
    phase: &'static str,
    error: &NacelleError,
    include_opcode: bool,
) -> Vec<opentelemetry::KeyValue> {
    let mut attributes = tcp_phase_attributes(context, phase, include_opcode);
    attributes.push(opentelemetry::KeyValue::new(
        "error_kind",
        error_kind(error),
    ));
    attributes
}

#[cfg(feature = "otel")]
fn error_kind(error: &NacelleError) -> &'static str {
    match error {
        NacelleError::ResourceLimit(_) => "resource_limit",
        NacelleError::Timeout(_) => "timeout",
        NacelleError::InvalidFrame(_) => "invalid_frame",
        NacelleError::FrameTooLarge { .. } => "frame_too_large",
        NacelleError::UnexpectedEof => "unexpected_eof",
        NacelleError::ConnectionClosed => "connection_closed",
        NacelleError::MissingProtocol => "missing_protocol",
        NacelleError::Io(_) => "io",
        NacelleError::Protocol(_) => "protocol",
        NacelleError::Handler(_) => "handler",
        NacelleError::Join(_) => "join",
    }
}

#[cfg(feature = "otel")]
#[derive(Debug)]
struct NacelleTcpOtelMetrics {
    connection_active: opentelemetry::metrics::UpDownCounter<i64>,
    connection_accepted: opentelemetry::metrics::Counter<u64>,
    connection_closed: opentelemetry::metrics::Counter<u64>,
    request_in_flight: opentelemetry::metrics::UpDownCounter<i64>,
    request_started: opentelemetry::metrics::Counter<u64>,
    request_completed: opentelemetry::metrics::Counter<u64>,
    request_bytes: opentelemetry::metrics::Counter<u64>,
    request_body_bytes: opentelemetry::metrics::Counter<u64>,
    response_bytes: opentelemetry::metrics::Counter<u64>,
    request_duration_ms: opentelemetry::metrics::Histogram<f64>,
    phase_duration_ms: opentelemetry::metrics::Histogram<f64>,
    errors: opentelemetry::metrics::Counter<u64>,
    resource_limit_rejections: opentelemetry::metrics::Counter<u64>,
}

#[cfg(feature = "otel")]
impl NacelleTcpOtelMetrics {
    fn new() -> Self {
        let meter = opentelemetry::global::meter("nacelle.tcp");
        Self {
            connection_active: meter
                .i64_up_down_counter("nacelle.tcp.connections.active")
                .build(),
            connection_accepted: meter
                .u64_counter("nacelle.tcp.connections.accepted")
                .build(),
            connection_closed: meter.u64_counter("nacelle.tcp.connections.closed").build(),
            request_in_flight: meter
                .i64_up_down_counter("nacelle.tcp.requests.in_flight")
                .build(),
            request_started: meter.u64_counter("nacelle.tcp.requests.started").build(),
            request_completed: meter.u64_counter("nacelle.tcp.requests.completed").build(),
            request_bytes: meter.u64_counter("nacelle.tcp.request.bytes").build(),
            request_body_bytes: meter.u64_counter("nacelle.tcp.request.body_bytes").build(),
            response_bytes: meter.u64_counter("nacelle.tcp.response.bytes").build(),
            request_duration_ms: meter
                .f64_histogram("nacelle.tcp.request.duration_ms")
                .build(),
            phase_duration_ms: meter.f64_histogram("nacelle.tcp.phase.duration_ms").build(),
            errors: meter.u64_counter("nacelle.tcp.errors").build(),
            resource_limit_rejections: meter
                .u64_counter("nacelle.tcp.resource_limit.rejections")
                .build(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn telemetry_config_keeps_opcode_labels_opt_in() {
        let telemetry = NacelleTcpTelemetry::default();

        assert!(telemetry.config().metrics);
        assert!(!telemetry.config().opcode_labels);

        let telemetry = telemetry.with_opcode_labels(true);

        assert!(telemetry.config().opcode_labels);
    }
}
