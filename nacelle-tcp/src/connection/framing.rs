use bytes::BytesMut;

use crate::protocol::{DecodedRequest, Protocol};
use crate::telemetry::{NacelleTcpMetricsContext, NacelleTcpTelemetry};
use nacelle_core::config::NacelleConfig;
use nacelle_core::error::NacelleError;
use nacelle_core::limits::{MemoryReservation, NacelleRuntimeState};
use nacelle_core::request::RequestMetadata;

use super::metrics::{finish_tcp_phase, start_tcp_phase};

pub(super) fn reserve_connection_buffers(
    config: &NacelleConfig,
    runtime_state: &NacelleRuntimeState,
) -> Result<MemoryReservation, NacelleError> {
    let bytes = config
        .read_buffer_capacity
        .saturating_add(config.response_buffer_capacity);
    runtime_state.reserve_memory(bytes)
}

pub(super) fn decode_next_request<Req, P>(
    protocol: &P,
    read_buf: &mut BytesMut,
    max_frame_len: usize,
    tcp_telemetry: &NacelleTcpTelemetry,
    metrics_context: &NacelleTcpMetricsContext,
) -> Result<Option<DecodedRequest<Req>>, NacelleError>
where
    Req: RequestMetadata + Send + 'static,
    P: Protocol<Req> + Send + Sync + 'static,
{
    let decode_started = start_tcp_phase(tcp_telemetry);
    let result = protocol.decode_head(read_buf, max_frame_len);
    finish_tcp_phase(
        tcp_telemetry,
        Some(metrics_context),
        "decode",
        decode_started,
    );
    if let Err(error) = &result {
        tcp_telemetry.error(metrics_context, "decode", error);
    }
    result
}
