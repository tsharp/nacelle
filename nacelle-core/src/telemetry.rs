use std::time::Duration;

#[cfg(feature = "otel")]
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NacelleTransport {
    Tcp,
    UnixSocket,
    Http,
}

impl NacelleTransport {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Tcp => "tcp",
            Self::UnixSocket => "unix_socket",
            Self::Http => "http",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NacelleTelemetryEventKind {
    ListenerConfigured,
    ListenerFailed,
    ConnectionOpened,
    ConnectionRejected,
    RequestRejected,
    RequestCompleted,
    RequestFailed,
    ResponseBodyBytes,
    Timeout,
    ShutdownRequested,
    ListenerStoppedAccepting,
    DrainStarted,
    DrainCompleted,
    DrainTimedOut,
    ConnectionsAborted,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct NacelleTelemetryEvent {
    pub kind: NacelleTelemetryEventKind,
    pub transport: Option<NacelleTransport>,
    pub reason: Option<&'static str>,
    pub count: u64,
}

pub trait NacelleTelemetrySink: Send + Sync + 'static {
    fn record(&self, event: NacelleTelemetryEvent);
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct NacelleTelemetryConfig {
    pub tcp_metrics: bool,
    pub tcp_opcode_labels: bool,
}

impl Default for NacelleTelemetryConfig {
    fn default() -> Self {
        Self {
            tcp_metrics: true,
            tcp_opcode_labels: false,
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

    pub fn without_opcode(&self) -> Self {
        Self {
            transport: self.transport,
            listener: self.listener.clone(),
            protocol: self.protocol,
            tls: self.tls,
            opcode: None,
        }
    }
}

#[derive(Debug, Default)]
pub struct NacelleInMemoryTelemetrySink {
    events: Mutex<Vec<NacelleTelemetryEvent>>,
}

impl NacelleInMemoryTelemetrySink {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn events(&self) -> Vec<NacelleTelemetryEvent> {
        self.events.lock().expect("telemetry sink poisoned").clone()
    }
}

impl NacelleTelemetrySink for NacelleInMemoryTelemetrySink {
    fn record(&self, event: NacelleTelemetryEvent) {
        self.events
            .lock()
            .expect("telemetry sink poisoned")
            .push(event);
    }
}

#[derive(Clone)]
pub struct NacelleTelemetry {
    sink: Option<Arc<dyn NacelleTelemetrySink>>,
    config: NacelleTelemetryConfig,
    #[cfg(feature = "otel")]
    metrics: std::sync::Arc<OtelMetrics>,
}

impl std::fmt::Debug for NacelleTelemetry {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("NacelleTelemetry")
            .field("has_sink", &self.sink.is_some())
            .field("config", &self.config)
            .finish()
    }
}

impl Default for NacelleTelemetry {
    fn default() -> Self {
        Self::new()
    }
}

impl NacelleTelemetry {
    pub fn new() -> Self {
        Self {
            sink: None,
            config: NacelleTelemetryConfig::default(),
            #[cfg(feature = "otel")]
            metrics: std::sync::Arc::new(OtelMetrics::new()),
        }
    }

    pub fn with_sink(mut self, sink: Arc<dyn NacelleTelemetrySink>) -> Self {
        self.sink = Some(sink);
        self
    }

    pub fn with_config(mut self, config: NacelleTelemetryConfig) -> Self {
        self.config = config;
        self
    }

    pub fn with_tcp_metrics(mut self, enabled: bool) -> Self {
        self.config.tcp_metrics = enabled;
        self
    }

    pub fn with_tcp_opcode_labels(mut self, enabled: bool) -> Self {
        self.config.tcp_opcode_labels = enabled;
        self
    }

    pub fn config(&self) -> NacelleTelemetryConfig {
        self.config
    }

    pub fn tcp_metrics_enabled(&self) -> bool {
        cfg!(feature = "otel") && self.config.tcp_metrics
    }

    pub fn listener_configured(&self, transport: NacelleTransport, name: &str, addr: &str) {
        tracing::info!(
            target: "nacelle",
            transport = transport.as_str(),
            binding = name,
            addr,
            "listener configured"
        );
        self.record(NacelleTelemetryEvent {
            kind: NacelleTelemetryEventKind::ListenerConfigured,
            transport: Some(transport),
            reason: None,
            count: 1,
        });
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
        self.record(NacelleTelemetryEvent {
            kind: NacelleTelemetryEventKind::ListenerFailed,
            transport: Some(transport),
            reason: error_reason(error),
            count: 1,
        });
    }

    pub fn connection_opened(&self, transport: NacelleTransport) {
        tracing::debug!(
            target: "nacelle",
            transport = transport.as_str(),
            "connection opened"
        );
        self.record(NacelleTelemetryEvent {
            kind: NacelleTelemetryEventKind::ConnectionOpened,
            transport: Some(transport),
            reason: None,
            count: 1,
        });
        #[cfg(feature = "otel")]
        self.metrics.connection_count.add(
            1,
            &[opentelemetry::KeyValue::new(
                "transport",
                transport.as_str(),
            )],
        );
    }

    pub fn connection_rejected(&self, transport: NacelleTransport, reason: &'static str) {
        tracing::warn!(
            target: "nacelle",
            transport = transport.as_str(),
            reason,
            "connection rejected"
        );
        self.record(NacelleTelemetryEvent {
            kind: NacelleTelemetryEventKind::ConnectionRejected,
            transport: Some(transport),
            reason: Some(reason),
            count: 1,
        });
        #[cfg(feature = "otel")]
        self.metrics.rejection_count.add(
            1,
            &[
                opentelemetry::KeyValue::new("transport", transport.as_str()),
                opentelemetry::KeyValue::new("reason", reason),
            ],
        );
    }

    pub fn request_rejected(&self, transport: NacelleTransport, reason: &'static str) {
        tracing::warn!(
            target: "nacelle",
            transport = transport.as_str(),
            reason,
            "request rejected"
        );
        self.record(NacelleTelemetryEvent {
            kind: NacelleTelemetryEventKind::RequestRejected,
            transport: Some(transport),
            reason: Some(reason),
            count: 1,
        });
        #[cfg(feature = "otel")]
        self.metrics.rejection_count.add(
            1,
            &[
                opentelemetry::KeyValue::new("transport", transport.as_str()),
                opentelemetry::KeyValue::new("reason", reason),
            ],
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
        self.record(NacelleTelemetryEvent {
            kind: NacelleTelemetryEventKind::RequestCompleted,
            transport: Some(transport),
            reason: None,
            count: 1,
        });
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
        self.record(NacelleTelemetryEvent {
            kind: NacelleTelemetryEventKind::RequestFailed,
            transport: Some(transport),
            reason: error_reason(error),
            count: 1,
        });
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

    pub fn timeout(&self, transport: NacelleTransport, operation: &'static str) {
        self.record(NacelleTelemetryEvent {
            kind: NacelleTelemetryEventKind::Timeout,
            transport: Some(transport),
            reason: Some(operation),
            count: 1,
        });
        #[cfg(feature = "otel")]
        self.metrics.timeout_count.add(
            1,
            &[
                opentelemetry::KeyValue::new("transport", transport.as_str()),
                opentelemetry::KeyValue::new("operation", operation),
            ],
        );
    }

    pub fn shutdown_event(&self, kind: NacelleTelemetryEventKind, transport: NacelleTransport) {
        self.record(NacelleTelemetryEvent {
            kind,
            transport: Some(transport),
            reason: None,
            count: 1,
        });
        #[cfg(feature = "otel")]
        self.metrics.shutdown_event_count.add(
            1,
            &[
                opentelemetry::KeyValue::new("transport", transport.as_str()),
                opentelemetry::KeyValue::new("stage", shutdown_stage(kind)),
            ],
        );
    }

    pub fn shutdown_requested(&self) {
        self.record(NacelleTelemetryEvent {
            kind: NacelleTelemetryEventKind::ShutdownRequested,
            transport: None,
            reason: None,
            count: 1,
        });
        #[cfg(feature = "otel")]
        self.metrics.shutdown_event_count.add(
            1,
            &[
                opentelemetry::KeyValue::new("transport", "host"),
                opentelemetry::KeyValue::new("stage", "requested"),
            ],
        );
    }

    pub fn connections_aborted(&self, transport: NacelleTransport, count: usize) {
        self.record(NacelleTelemetryEvent {
            kind: NacelleTelemetryEventKind::ConnectionsAborted,
            transport: Some(transport),
            reason: None,
            count: count as u64,
        });
        #[cfg(feature = "otel")]
        self.metrics.connection_abort_count.add(
            count as u64,
            &[opentelemetry::KeyValue::new(
                "transport",
                transport.as_str(),
            )],
        );
    }

    pub fn response_body_bytes(&self, transport: NacelleTransport, bytes: usize) {
        if bytes == 0 {
            return;
        }
        self.record(NacelleTelemetryEvent {
            kind: NacelleTelemetryEventKind::ResponseBodyBytes,
            transport: Some(transport),
            reason: None,
            count: bytes as u64,
        });
        #[cfg(feature = "otel")]
        self.metrics.response_bytes.add(
            bytes as u64,
            &[opentelemetry::KeyValue::new(
                "transport",
                transport.as_str(),
            )],
        );
    }

    pub fn register_runtime_state(&self, state: crate::limits::NacelleRuntimeState) {
        #[cfg(feature = "otel")]
        self.metrics.register_runtime_state(state);
        #[cfg(not(feature = "otel"))]
        let _ = state;
    }

    pub fn tcp_connection_accepted(&self, context: &NacelleTcpMetricsContext) {
        if !self.tcp_metrics_enabled() {
            return;
        }
        #[cfg(feature = "otel")]
        {
            let attributes = tcp_connection_attributes(context);
            self.metrics.tcp_connection_accepted.add(1, &attributes);
            self.metrics.tcp_connection_active.add(1, &attributes);
        }
    }

    pub fn tcp_connection_closed(
        &self,
        context: &NacelleTcpMetricsContext,
        close_reason: &'static str,
    ) {
        if !self.tcp_metrics_enabled() {
            return;
        }
        #[cfg(feature = "otel")]
        {
            let mut attributes = tcp_connection_attributes(context);
            attributes.push(opentelemetry::KeyValue::new("close_reason", close_reason));
            self.metrics.tcp_connection_closed.add(1, &attributes);
            let active_attributes = tcp_connection_attributes(context);
            self.metrics
                .tcp_connection_active
                .add(-1, &active_attributes);
        }
    }

    pub fn tcp_request_started(&self, context: &NacelleTcpMetricsContext) {
        if !self.tcp_metrics_enabled() {
            return;
        }
        #[cfg(feature = "otel")]
        {
            let attributes = tcp_request_attributes(context, self.config.tcp_opcode_labels);
            self.metrics.tcp_request_started.add(1, &attributes);
            self.metrics.tcp_request_in_flight.add(1, &attributes);
        }
    }

    pub fn tcp_request_completed(
        &self,
        context: &NacelleTcpMetricsContext,
        status: &'static str,
        request_bytes: usize,
        response_bytes: usize,
        elapsed: Duration,
    ) {
        if !self.tcp_metrics_enabled() {
            return;
        }
        #[cfg(feature = "otel")]
        {
            let mut attributes = tcp_request_attributes(context, self.config.tcp_opcode_labels);
            attributes.push(opentelemetry::KeyValue::new("status", status));
            self.metrics.tcp_request_completed.add(1, &attributes);
            self.metrics
                .tcp_request_duration_ms
                .record(elapsed.as_secs_f64() * 1_000.0, &attributes);
            if request_bytes != 0 {
                self.metrics
                    .tcp_request_bytes
                    .add(request_bytes as u64, &attributes);
            }
            if response_bytes != 0 {
                self.metrics
                    .tcp_response_bytes
                    .add(response_bytes as u64, &attributes);
            }

            let active_attributes = tcp_request_attributes(context, self.config.tcp_opcode_labels);
            self.metrics
                .tcp_request_in_flight
                .add(-1, &active_attributes);
        }
    }

    pub fn tcp_request_body_bytes(&self, context: &NacelleTcpMetricsContext, bytes: usize) {
        if bytes == 0 || !self.tcp_metrics_enabled() {
            return;
        }
        #[cfg(feature = "otel")]
        self.metrics.tcp_request_body_bytes.add(
            bytes as u64,
            &tcp_request_attributes(context, self.config.tcp_opcode_labels),
        );
    }

    pub fn tcp_phase_duration(
        &self,
        context: &NacelleTcpMetricsContext,
        phase: &'static str,
        elapsed: Duration,
    ) {
        if !self.tcp_metrics_enabled() {
            return;
        }
        #[cfg(feature = "otel")]
        self.metrics.tcp_phase_duration_ms.record(
            elapsed.as_secs_f64() * 1_000.0,
            &tcp_phase_attributes(context, phase, self.config.tcp_opcode_labels),
        );
    }

    pub fn tcp_error(
        &self,
        context: &NacelleTcpMetricsContext,
        phase: &'static str,
        error: &crate::error::NacelleError,
    ) {
        if !self.tcp_metrics_enabled() {
            return;
        }
        #[cfg(feature = "otel")]
        {
            self.metrics.tcp_errors.add(
                1,
                &tcp_error_attributes(context, phase, error, self.config.tcp_opcode_labels),
            );
            if let crate::error::NacelleError::ResourceLimit(limit) = error {
                let mut attributes = tcp_request_attributes(context, self.config.tcp_opcode_labels);
                attributes.push(opentelemetry::KeyValue::new("limit", *limit));
                attributes.push(opentelemetry::KeyValue::new("phase", phase));
                self.metrics
                    .tcp_resource_limit_rejections
                    .add(1, &attributes);
            }
        }
    }

    fn record(&self, event: NacelleTelemetryEvent) {
        if let Some(sink) = &self.sink {
            sink.record(event);
        }
    }
}

fn error_reason(error: &crate::error::NacelleError) -> Option<&'static str> {
    match error {
        crate::error::NacelleError::ResourceLimit(reason)
        | crate::error::NacelleError::Timeout(reason)
        | crate::error::NacelleError::InvalidFrame(reason) => Some(reason),
        crate::error::NacelleError::FrameTooLarge { .. } => Some("frame_too_large"),
        crate::error::NacelleError::UnexpectedEof => Some("unexpected_eof"),
        crate::error::NacelleError::ConnectionClosed => Some("connection_closed"),
        crate::error::NacelleError::MissingProtocol => Some("missing_protocol"),
        crate::error::NacelleError::Io(_) => Some("io"),
        crate::error::NacelleError::Protocol(_) => Some("protocol"),
        crate::error::NacelleError::Handler(_) => Some("handler"),
        crate::error::NacelleError::Join(_) => Some("join"),
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
    error: &crate::error::NacelleError,
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
fn error_kind(error: &crate::error::NacelleError) -> &'static str {
    match error {
        crate::error::NacelleError::ResourceLimit(_) => "resource_limit",
        crate::error::NacelleError::Timeout(_) => "timeout",
        crate::error::NacelleError::InvalidFrame(_) => "invalid_frame",
        crate::error::NacelleError::FrameTooLarge { .. } => "frame_too_large",
        crate::error::NacelleError::UnexpectedEof => "unexpected_eof",
        crate::error::NacelleError::ConnectionClosed => "connection_closed",
        crate::error::NacelleError::MissingProtocol => "missing_protocol",
        crate::error::NacelleError::Io(_) => "io",
        crate::error::NacelleError::Protocol(_) => "protocol",
        crate::error::NacelleError::Handler(_) => "handler",
        crate::error::NacelleError::Join(_) => "join",
    }
}

#[cfg(feature = "otel")]
fn shutdown_stage(kind: NacelleTelemetryEventKind) -> &'static str {
    match kind {
        NacelleTelemetryEventKind::ShutdownRequested => "requested",
        NacelleTelemetryEventKind::ListenerStoppedAccepting => "listener_stopped_accepting",
        NacelleTelemetryEventKind::DrainStarted => "drain_started",
        NacelleTelemetryEventKind::DrainCompleted => "drain_completed",
        NacelleTelemetryEventKind::DrainTimedOut => "drain_timed_out",
        NacelleTelemetryEventKind::ConnectionsAborted => "connections_aborted",
        _ => "other",
    }
}

#[cfg(feature = "otel")]
#[derive(Debug)]
struct OtelMetrics {
    runtime_state_registered: AtomicBool,
    runtime_state_gauges: Mutex<Vec<opentelemetry::metrics::ObservableGauge<u64>>>,
    connection_count: opentelemetry::metrics::Counter<u64>,
    request_count: opentelemetry::metrics::Counter<u64>,
    request_error_count: opentelemetry::metrics::Counter<u64>,
    rejection_count: opentelemetry::metrics::Counter<u64>,
    timeout_count: opentelemetry::metrics::Counter<u64>,
    shutdown_event_count: opentelemetry::metrics::Counter<u64>,
    connection_abort_count: opentelemetry::metrics::Counter<u64>,
    request_bytes: opentelemetry::metrics::Counter<u64>,
    response_bytes: opentelemetry::metrics::Counter<u64>,
    request_duration_ms: opentelemetry::metrics::Histogram<f64>,
    tcp_connection_active: opentelemetry::metrics::UpDownCounter<i64>,
    tcp_connection_accepted: opentelemetry::metrics::Counter<u64>,
    tcp_connection_closed: opentelemetry::metrics::Counter<u64>,
    tcp_request_in_flight: opentelemetry::metrics::UpDownCounter<i64>,
    tcp_request_started: opentelemetry::metrics::Counter<u64>,
    tcp_request_completed: opentelemetry::metrics::Counter<u64>,
    tcp_request_bytes: opentelemetry::metrics::Counter<u64>,
    tcp_request_body_bytes: opentelemetry::metrics::Counter<u64>,
    tcp_response_bytes: opentelemetry::metrics::Counter<u64>,
    tcp_request_duration_ms: opentelemetry::metrics::Histogram<f64>,
    tcp_phase_duration_ms: opentelemetry::metrics::Histogram<f64>,
    tcp_errors: opentelemetry::metrics::Counter<u64>,
    tcp_resource_limit_rejections: opentelemetry::metrics::Counter<u64>,
}

#[cfg(feature = "otel")]
impl OtelMetrics {
    fn new() -> Self {
        let meter = opentelemetry::global::meter("nacelle");
        Self {
            runtime_state_registered: AtomicBool::new(false),
            runtime_state_gauges: Mutex::new(Vec::new()),
            connection_count: meter.u64_counter("nacelle.connections").build(),
            request_count: meter.u64_counter("nacelle.requests").build(),
            request_error_count: meter.u64_counter("nacelle.request_errors").build(),
            rejection_count: meter.u64_counter("nacelle.rejections").build(),
            timeout_count: meter.u64_counter("nacelle.timeouts").build(),
            shutdown_event_count: meter.u64_counter("nacelle.shutdown_events").build(),
            connection_abort_count: meter.u64_counter("nacelle.connection_aborts").build(),
            request_bytes: meter.u64_counter("nacelle.request_bytes").build(),
            response_bytes: meter.u64_counter("nacelle.response_bytes").build(),
            request_duration_ms: meter.f64_histogram("nacelle.request_duration_ms").build(),
            tcp_connection_active: meter
                .i64_up_down_counter("nacelle.tcp.connections.active")
                .build(),
            tcp_connection_accepted: meter
                .u64_counter("nacelle.tcp.connections.accepted")
                .build(),
            tcp_connection_closed: meter.u64_counter("nacelle.tcp.connections.closed").build(),
            tcp_request_in_flight: meter
                .i64_up_down_counter("nacelle.tcp.requests.in_flight")
                .build(),
            tcp_request_started: meter.u64_counter("nacelle.tcp.requests.started").build(),
            tcp_request_completed: meter.u64_counter("nacelle.tcp.requests.completed").build(),
            tcp_request_bytes: meter.u64_counter("nacelle.tcp.request.bytes").build(),
            tcp_request_body_bytes: meter.u64_counter("nacelle.tcp.request.body_bytes").build(),
            tcp_response_bytes: meter.u64_counter("nacelle.tcp.response.bytes").build(),
            tcp_request_duration_ms: meter
                .f64_histogram("nacelle.tcp.request.duration_ms")
                .build(),
            tcp_phase_duration_ms: meter.f64_histogram("nacelle.tcp.phase.duration_ms").build(),
            tcp_errors: meter.u64_counter("nacelle.tcp.errors").build(),
            tcp_resource_limit_rejections: meter
                .u64_counter("nacelle.tcp.resource_limit.rejections")
                .build(),
        }
    }

    fn register_runtime_state(&self, state: crate::limits::NacelleRuntimeState) {
        if self.runtime_state_registered.swap(true, Ordering::AcqRel) {
            return;
        }

        let meter = opentelemetry::global::meter("nacelle");
        let connections = state.clone();
        let requests = state.clone();
        let streaming = state.clone();
        let memory = state;
        let gauges = vec![
            meter
                .u64_observable_gauge("nacelle.connections.active")
                .with_callback(move |observer| {
                    observer.observe(connections.active_connections() as u64, &[])
                })
                .build(),
            meter
                .u64_observable_gauge("nacelle.requests.active")
                .with_callback(move |observer| {
                    observer.observe(requests.active_requests() as u64, &[])
                })
                .build(),
            meter
                .u64_observable_gauge("nacelle.streaming_tasks.active")
                .with_callback(move |observer| {
                    observer.observe(streaming.active_streaming_tasks() as u64, &[])
                })
                .build(),
            meter
                .u64_observable_gauge("nacelle.memory.used_bytes")
                .with_callback(move |observer| {
                    observer.observe(memory.memory_used_bytes() as u64, &[])
                })
                .build(),
        ];
        self.runtime_state_gauges
            .lock()
            .expect("otel gauge registry poisoned")
            .extend(gauges);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn in_memory_sink_records_rejection_timeout_and_shutdown_events() {
        let sink = Arc::new(NacelleInMemoryTelemetrySink::new());
        let telemetry = NacelleTelemetry::new().with_sink(sink.clone());

        telemetry.connection_rejected(NacelleTransport::Tcp, "connections");
        telemetry.request_rejected(NacelleTransport::Http, "host");
        telemetry.timeout(NacelleTransport::Tcp, "request_body_read");
        telemetry.shutdown_requested();
        telemetry.shutdown_event(
            NacelleTelemetryEventKind::DrainCompleted,
            NacelleTransport::Tcp,
        );

        let events = sink.events();
        assert_eq!(
            events.iter().map(|event| event.kind).collect::<Vec<_>>(),
            vec![
                NacelleTelemetryEventKind::ConnectionRejected,
                NacelleTelemetryEventKind::RequestRejected,
                NacelleTelemetryEventKind::Timeout,
                NacelleTelemetryEventKind::ShutdownRequested,
                NacelleTelemetryEventKind::DrainCompleted,
            ]
        );
        assert_eq!(events[0].reason, Some("connections"));
        assert_eq!(events[1].reason, Some("host"));
        assert_eq!(events[2].reason, Some("request_body_read"));
        assert_eq!(events[3].transport, None);
    }

    #[test]
    fn telemetry_config_keeps_opcode_labels_opt_in() {
        let telemetry = NacelleTelemetry::default();

        assert!(telemetry.config().tcp_metrics);
        assert!(!telemetry.config().tcp_opcode_labels);

        let telemetry = telemetry.with_tcp_opcode_labels(true);

        assert!(telemetry.config().tcp_opcode_labels);
    }
}
