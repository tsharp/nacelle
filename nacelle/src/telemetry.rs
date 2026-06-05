use std::time::Duration;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NacelleTransport {
    RawTcp,
    Http,
}

impl NacelleTransport {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::RawTcp => "raw_tcp",
            Self::Http => "http",
        }
    }
}

#[derive(Debug, Clone)]
pub struct NacelleTelemetry {
    #[cfg(feature = "otel")]
    metrics: std::sync::Arc<OtelMetrics>,
}

impl Default for NacelleTelemetry {
    fn default() -> Self {
        Self::new()
    }
}

impl NacelleTelemetry {
    pub fn new() -> Self {
        Self {
            #[cfg(feature = "otel")]
            metrics: std::sync::Arc::new(OtelMetrics::new()),
        }
    }

    pub fn listener_configured(&self, transport: NacelleTransport, name: &str, addr: &str) {
        tracing::info!(
            target: "nacelle",
            transport = transport.as_str(),
            binding = name,
            addr,
            "listener configured"
        );
    }

    pub fn listener_failed(
        &self,
        transport: NacelleTransport,
        name: &str,
        addr: &str,
        error: &crate::error::NacelleError,
    ) {
        tracing::error!(
            target: "nacelle",
            transport = transport.as_str(),
            binding = name,
            addr,
            error = %error,
            "listener failed"
        );
    }

    pub fn connection_opened(&self, transport: NacelleTransport) {
        tracing::debug!(
            target: "nacelle",
            transport = transport.as_str(),
            "connection opened"
        );
        #[cfg(feature = "otel")]
        self.metrics.connection_count.add(
            1,
            &[opentelemetry::KeyValue::new(
                "transport",
                transport.as_str(),
            )],
        );
    }

    pub fn request_completed(
        &self,
        transport: NacelleTransport,
        opcode: Option<u64>,
        request_bytes: usize,
        response_bytes: usize,
        elapsed: Duration,
    ) {
        tracing::debug!(
            target: "nacelle",
            transport = transport.as_str(),
            opcode,
            request_bytes,
            response_bytes,
            elapsed_us = elapsed.as_micros() as u64,
            "request completed"
        );
        #[cfg(feature = "otel")]
        {
            let attributes = [opentelemetry::KeyValue::new(
                "transport",
                transport.as_str(),
            )];
            self.metrics.request_count.add(1, &attributes);
            self.metrics
                .request_duration_ms
                .record(elapsed.as_secs_f64() * 1_000.0, &attributes);
            self.metrics
                .request_bytes
                .add(request_bytes as u64, &attributes);
            self.metrics
                .response_bytes
                .add(response_bytes as u64, &attributes);
        }
    }

    pub fn request_failed(
        &self,
        transport: NacelleTransport,
        opcode: Option<u64>,
        elapsed: Duration,
        error: &crate::error::NacelleError,
    ) {
        tracing::warn!(
            target: "nacelle",
            transport = transport.as_str(),
            opcode,
            elapsed_us = elapsed.as_micros() as u64,
            error = %error,
            "request failed"
        );
        #[cfg(feature = "otel")]
        {
            let attributes = [opentelemetry::KeyValue::new(
                "transport",
                transport.as_str(),
            )];
            self.metrics.request_error_count.add(1, &attributes);
            self.metrics
                .request_duration_ms
                .record(elapsed.as_secs_f64() * 1_000.0, &attributes);
        }
    }
}

#[cfg(feature = "otel")]
#[derive(Debug)]
struct OtelMetrics {
    connection_count: opentelemetry::metrics::Counter<u64>,
    request_count: opentelemetry::metrics::Counter<u64>,
    request_error_count: opentelemetry::metrics::Counter<u64>,
    request_bytes: opentelemetry::metrics::Counter<u64>,
    response_bytes: opentelemetry::metrics::Counter<u64>,
    request_duration_ms: opentelemetry::metrics::Histogram<f64>,
}

#[cfg(feature = "otel")]
impl OtelMetrics {
    fn new() -> Self {
        let meter = opentelemetry::global::meter("nacelle");
        Self {
            connection_count: meter.u64_counter("nacelle.connections").build(),
            request_count: meter.u64_counter("nacelle.requests").build(),
            request_error_count: meter.u64_counter("nacelle.request_errors").build(),
            request_bytes: meter.u64_counter("nacelle.request_bytes").build(),
            response_bytes: meter.u64_counter("nacelle.response_bytes").build(),
            request_duration_ms: meter.f64_histogram("nacelle.request_duration_ms").build(),
        }
    }
}
