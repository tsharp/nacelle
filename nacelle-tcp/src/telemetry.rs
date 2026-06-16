use std::sync::Arc;
use std::time::Duration;

use nacelle_core::error::NacelleError;
use nacelle_core::telemetry::NacelleTransport;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct NacelleTcpTelemetryConfig {
    pub metrics: bool,
    pub opcode_labels: bool,
    pub request_start_metrics: bool,
    pub request_duration_metrics: bool,
    pub request_byte_metrics: bool,
    pub active_request_metrics: bool,
    pub phase_duration_metrics: bool,
}

impl Default for NacelleTcpTelemetryConfig {
    fn default() -> Self {
        Self {
            metrics: true,
            opcode_labels: false,
            request_start_metrics: false,
            request_duration_metrics: false,
            request_byte_metrics: false,
            active_request_metrics: false,
            phase_duration_metrics: false,
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
    #[cfg(feature = "otel")]
    connection_attributes: Arc<[opentelemetry::KeyValue]>,
    #[cfg(feature = "otel")]
    request_attributes: Arc<[opentelemetry::KeyValue]>,
    #[cfg(feature = "otel")]
    request_ok_attributes: Arc<[opentelemetry::KeyValue]>,
    #[cfg(feature = "otel")]
    request_error_attributes: Arc<[opentelemetry::KeyValue]>,
}

impl NacelleTcpMetricsContext {
    pub fn new(
        transport: NacelleTransport,
        listener: Arc<str>,
        protocol: &'static str,
        tls: &'static str,
        opcode: Option<u64>,
    ) -> Self {
        Self::with_opcode_labels(transport, listener, protocol, tls, opcode, false)
    }

    pub(crate) fn with_opcode_labels(
        transport: NacelleTransport,
        listener: Arc<str>,
        protocol: &'static str,
        tls: &'static str,
        opcode: Option<u64>,
        include_opcode: bool,
    ) -> Self {
        #[cfg(not(feature = "otel"))]
        let _ = include_opcode;

        #[cfg(feature = "otel")]
        let connection_attributes = tcp_connection_attributes(transport, &listener, tls);
        #[cfg(feature = "otel")]
        let request_attributes = tcp_request_attributes(
            connection_attributes.as_ref(),
            protocol,
            opcode,
            include_opcode,
        );
        #[cfg(feature = "otel")]
        let request_ok_attributes = attributes_with_key_value(
            request_attributes.as_ref(),
            opentelemetry::KeyValue::new("status", "ok"),
        );
        #[cfg(feature = "otel")]
        let request_error_attributes = attributes_with_key_value(
            request_attributes.as_ref(),
            opentelemetry::KeyValue::new("status", "error"),
        );

        Self {
            transport,
            listener,
            protocol,
            tls,
            opcode,
            #[cfg(feature = "otel")]
            connection_attributes,
            #[cfg(feature = "otel")]
            request_attributes,
            #[cfg(feature = "otel")]
            request_ok_attributes,
            #[cfg(feature = "otel")]
            request_error_attributes,
        }
    }

    #[cfg(feature = "otel")]
    fn connection_attributes(&self) -> &[opentelemetry::KeyValue] {
        &self.connection_attributes
    }

    #[cfg(feature = "otel")]
    fn request_attributes(&self) -> &[opentelemetry::KeyValue] {
        &self.request_attributes
    }

    #[cfg(feature = "otel")]
    fn request_status_attributes(&self, status: &'static str) -> &[opentelemetry::KeyValue] {
        match status {
            "ok" => &self.request_ok_attributes,
            "error" => &self.request_error_attributes,
            _ => &self.request_attributes,
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

    pub fn with_request_start_metrics(mut self, enabled: bool) -> Self {
        self.config.request_start_metrics = enabled;
        self
    }

    pub fn with_request_duration_metrics(mut self, enabled: bool) -> Self {
        self.config.request_duration_metrics = enabled;
        self
    }

    pub fn with_request_byte_metrics(mut self, enabled: bool) -> Self {
        self.config.request_byte_metrics = enabled;
        self
    }

    pub fn with_active_request_metrics(mut self, enabled: bool) -> Self {
        self.config.active_request_metrics = enabled;
        self
    }

    pub fn with_phase_duration_metrics(mut self, enabled: bool) -> Self {
        self.config.phase_duration_metrics = enabled;
        self
    }

    pub fn config(&self) -> NacelleTcpTelemetryConfig {
        self.config
    }

    pub fn metrics_enabled(&self) -> bool {
        cfg!(feature = "otel") && self.config.metrics
    }

    pub fn request_metrics_enabled(&self) -> bool {
        self.metrics_enabled()
    }

    pub fn request_duration_metrics_enabled(&self) -> bool {
        self.metrics_enabled() && self.config.request_duration_metrics
    }

    pub fn phase_duration_metrics_enabled(&self) -> bool {
        self.metrics_enabled() && self.config.phase_duration_metrics
    }

    pub fn connection_accepted(&self, context: &NacelleTcpMetricsContext) {
        let _ = context;
        if !self.metrics_enabled() {
            return;
        }
        #[cfg(feature = "otel")]
        {
            let attributes = context.connection_attributes();
            self.metrics.connection_accepted.add(1, attributes);
            self.metrics.connection_active.add(1, attributes);
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
            let mut attributes = context.connection_attributes().to_vec();
            attributes.push(opentelemetry::KeyValue::new("close_reason", close_reason));
            self.metrics.connection_closed.add(1, &attributes);
            self.metrics
                .connection_active
                .add(-1, context.connection_attributes());
        }
    }

    pub fn request_started(&self, context: &NacelleTcpMetricsContext) {
        let _ = context;
        if !self.metrics_enabled() {
            return;
        }
        #[cfg(feature = "otel")]
        {
            if !self.config.request_start_metrics && !self.config.active_request_metrics {
                return;
            }
            let attributes = context.request_attributes();
            if self.config.request_start_metrics {
                self.metrics.request_started.add(1, attributes);
            }
            if self.config.active_request_metrics {
                self.metrics.request_in_flight.add(1, attributes);
            }
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
            let attributes = context.request_status_attributes(status);
            self.metrics.request_completed.add(1, attributes);
            if self.config.request_duration_metrics {
                self.metrics
                    .request_duration_ms
                    .record(elapsed.as_secs_f64() * 1_000.0, attributes);
            }
            if self.config.request_byte_metrics && request_bytes != 0 {
                self.metrics
                    .request_bytes
                    .add(request_bytes as u64, attributes);
            }
            if self.config.request_byte_metrics && response_bytes != 0 {
                self.metrics
                    .response_bytes
                    .add(response_bytes as u64, attributes);
            }

            if self.config.active_request_metrics {
                self.metrics
                    .request_in_flight
                    .add(-1, context.request_attributes());
            }
        }
    }

    pub fn request_body_bytes(&self, context: &NacelleTcpMetricsContext, bytes: usize) {
        let _ = context;
        if bytes == 0 || !self.metrics_enabled() || !self.config.request_byte_metrics {
            return;
        }
        #[cfg(feature = "otel")]
        self.metrics
            .request_body_bytes
            .add(bytes as u64, context.request_attributes());
    }

    pub fn phase_duration(
        &self,
        context: &NacelleTcpMetricsContext,
        phase: &'static str,
        elapsed: Duration,
    ) {
        let _ = (context, phase, elapsed);
        if !self.phase_duration_metrics_enabled() {
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
                let mut attributes = context.request_attributes().to_vec();
                attributes.push(opentelemetry::KeyValue::new("limit", *limit));
                attributes.push(opentelemetry::KeyValue::new("phase", phase));
                self.metrics.resource_limit_rejections.add(1, &attributes);
            }
        }
    }
}

#[cfg(feature = "otel")]
fn tcp_connection_attributes(
    transport: NacelleTransport,
    listener: &Arc<str>,
    tls: &'static str,
) -> Arc<[opentelemetry::KeyValue]> {
    Arc::from(
        vec![
            opentelemetry::KeyValue::new("listener", listener.as_ref().to_owned()),
            opentelemetry::KeyValue::new("transport", transport.as_str()),
            opentelemetry::KeyValue::new("tls", tls),
        ]
        .into_boxed_slice(),
    )
}

#[cfg(feature = "otel")]
fn tcp_request_attributes(
    connection_attributes: &[opentelemetry::KeyValue],
    protocol: &'static str,
    opcode: Option<u64>,
    include_opcode: bool,
) -> Arc<[opentelemetry::KeyValue]> {
    let mut attributes = connection_attributes.to_vec();
    attributes.push(opentelemetry::KeyValue::new("protocol", protocol));
    if include_opcode && let Some(opcode) = opcode {
        attributes.push(opentelemetry::KeyValue::new("opcode", opcode.to_string()));
    }
    Arc::from(attributes.into_boxed_slice())
}

#[cfg(feature = "otel")]
fn attributes_with_key_value(
    attributes: &[opentelemetry::KeyValue],
    key_value: opentelemetry::KeyValue,
) -> Arc<[opentelemetry::KeyValue]> {
    let mut attributes = attributes.to_vec();
    attributes.push(key_value);
    Arc::from(attributes.into_boxed_slice())
}

#[cfg(feature = "otel")]
fn tcp_phase_attributes(
    context: &NacelleTcpMetricsContext,
    phase: &'static str,
    include_opcode: bool,
) -> Vec<opentelemetry::KeyValue> {
    let mut attributes = tcp_request_attributes(
        context.connection_attributes(),
        context.protocol,
        context.opcode,
        include_opcode,
    )
    .to_vec();
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
    fn telemetry_config_keeps_expensive_metrics_opt_in() {
        let telemetry = NacelleTcpTelemetry::default();

        assert!(telemetry.config().metrics);
        assert!(!telemetry.config().opcode_labels);
        assert!(!telemetry.config().request_start_metrics);
        assert!(!telemetry.config().request_duration_metrics);
        assert!(!telemetry.config().request_byte_metrics);
        assert!(!telemetry.config().active_request_metrics);
        assert!(!telemetry.config().phase_duration_metrics);

        let telemetry = telemetry
            .with_opcode_labels(true)
            .with_request_start_metrics(true)
            .with_request_duration_metrics(true)
            .with_request_byte_metrics(true)
            .with_active_request_metrics(true)
            .with_phase_duration_metrics(true);

        assert!(telemetry.config().opcode_labels);
        assert!(telemetry.config().request_start_metrics);
        assert!(telemetry.config().request_duration_metrics);
        assert!(telemetry.config().request_byte_metrics);
        assert!(telemetry.config().active_request_metrics);
        assert!(telemetry.config().phase_duration_metrics);
    }
}
