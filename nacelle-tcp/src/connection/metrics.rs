use std::time::{Duration, Instant};

use crate::protocol::Protocol;
use crate::telemetry::{NacelleTcpMetricsContext, NacelleTcpTelemetry};
use nacelle_core::error::NacelleError;
use nacelle_core::request::{NacelleConnectionMeta, RequestMetadata};
use nacelle_core::telemetry::{NacelleTelemetry, NacelleTransport};

pub(super) fn tcp_metrics_context<Req, P>(
    protocol: &P,
    connection: &NacelleConnectionMeta,
) -> NacelleTcpMetricsContext
where
    Req: RequestMetadata,
    P: Protocol<Req> + Send + Sync + 'static,
{
    NacelleTcpMetricsContext::new(
        connection.transport,
        connection.listener.clone(),
        protocol.name(),
        connection.tls_label(),
    )
}

pub(super) fn start_tcp_phase(tcp_telemetry: &NacelleTcpTelemetry) -> Option<Instant> {
    tcp_telemetry
        .phase_duration_metrics_enabled()
        .then(Instant::now)
}

pub(super) fn finish_tcp_phase(
    tcp_telemetry: &NacelleTcpTelemetry,
    metrics_context: Option<&NacelleTcpMetricsContext>,
    phase: &'static str,
    started: Option<Instant>,
) {
    if let (Some(started), Some(metrics_context)) = (started, metrics_context) {
        tcp_telemetry.phase_duration(metrics_context, phase, started.elapsed());
    }
}

pub(super) fn record_tcp_error(
    tcp_telemetry: &NacelleTcpTelemetry,
    metrics_context: Option<&NacelleTcpMetricsContext>,
    phase: &'static str,
    error: &NacelleError,
) {
    if let Some(metrics_context) = metrics_context {
        tcp_telemetry.error(metrics_context, phase, error);
    }
}

pub(super) fn record_core_request_completed(
    telemetry: &NacelleTelemetry,
    enabled: bool,
    transport: NacelleTransport,
    request_bytes: usize,
    response_bytes: usize,
    started: Option<Instant>,
) {
    if enabled {
        telemetry.request_completed(
            transport,
            request_bytes,
            response_bytes,
            elapsed_since(started),
        );
    }
}

pub(super) fn record_core_request_failed(
    telemetry: &NacelleTelemetry,
    enabled: bool,
    transport: NacelleTransport,
    started: Option<Instant>,
    error: &NacelleError,
) {
    if enabled {
        telemetry.request_failed(transport, elapsed_since(started), error);
    }
}

pub(super) fn tcp_close_reason(result: &Result<(), NacelleError>) -> &'static str {
    match result {
        Ok(()) => "eof",
        Err(NacelleError::Timeout(_)) => "timeout",
        Err(NacelleError::UnexpectedEof) => "unexpected_eof",
        Err(NacelleError::ConnectionClosed) => "connection_closed",
        Err(NacelleError::ResourceLimit(_)) => "resource_limit",
        Err(NacelleError::Io(_)) => "io",
        Err(NacelleError::Protocol(_)) => "protocol",
        Err(NacelleError::Handler(_)) => "handler",
        Err(NacelleError::Join(_)) => "join",
        Err(NacelleError::InvalidFrame(_)) | Err(NacelleError::FrameTooLarge { .. }) => {
            "invalid_frame"
        }
        Err(NacelleError::MissingProtocol) => "missing_protocol",
    }
}

fn elapsed_since(started: Option<Instant>) -> Duration {
    started.map_or(Duration::ZERO, |started| started.elapsed())
}

pub(super) struct TcpRequestMetricsGuard<'a> {
    telemetry: &'a NacelleTcpTelemetry,
    context: Option<NacelleTcpMetricsContext>,
    request_bytes: usize,
    started: Option<Instant>,
    completed: bool,
}

impl<'a> TcpRequestMetricsGuard<'a> {
    pub(super) fn new(
        telemetry: &'a NacelleTcpTelemetry,
        context: Option<NacelleTcpMetricsContext>,
        request_bytes: usize,
        started: Option<Instant>,
    ) -> Self {
        if let Some(context) = &context {
            telemetry.request_started(context);
        }
        Self {
            telemetry,
            context,
            request_bytes,
            started,
            completed: false,
        }
    }

    pub(super) fn complete(&mut self, status: &'static str, response_bytes: usize) {
        if let Some(context) = &self.context {
            self.telemetry.request_completed(
                context,
                status,
                self.request_bytes,
                response_bytes,
                elapsed_since(self.started),
            );
        }
        self.completed = true;
    }
}

impl Drop for TcpRequestMetricsGuard<'_> {
    fn drop(&mut self) {
        if !self.completed
            && let Some(context) = &self.context
        {
            self.telemetry.request_completed(
                context,
                "error",
                self.request_bytes,
                0,
                elapsed_since(self.started),
            );
        }
    }
}
