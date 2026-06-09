use tower::ServiceExt;

use crate::error::{BoxError, NacelleError};
use crate::handler::{Handler, handler_fn};
use crate::request::NacelleRequest;
use crate::response::NacelleResponse;

pub fn handler_from_tower_service<S, E>(service: S) -> impl Handler
where
    S: tower::Service<NacelleRequest, Response = NacelleResponse, Error = E>
        + Clone
        + Send
        + Sync
        + 'static,
    S::Future: Send + 'static,
    E: Into<BoxError> + 'static,
{
    handler_fn(move |request| {
        let service = service.clone();
        async move {
            service
                .oneshot(request)
                .await
                .map_err(|error| NacelleError::handler(error.into()))
        }
    })
}

#[cfg(test)]
mod tests {
    use bytes::Bytes;
    use tower::service_fn;

    use crate::request::{
        NacelleBody, NacelleConnectionMeta, NacelleRequest, NacelleRequestMeta, RawTcpRequestMeta,
    };

    use super::*;

    #[tokio::test]
    async fn wraps_tower_service_as_handler() {
        let service = service_fn(|mut request: NacelleRequest| async move {
            let mut body = Vec::new();
            while let Some(chunk) = request.body.next_chunk().await {
                body.extend_from_slice(&chunk?);
            }
            Ok::<_, NacelleError>(NacelleResponse::raw_tcp_bytes(Bytes::from(body)))
        });

        let handler = handler_from_tower_service::<_, NacelleError>(service);
        let response = handler
            .call(NacelleRequest {
                connection: NacelleConnectionMeta::raw_tcp(None, None),
                meta: NacelleRequestMeta::RawTcp(RawTcpRequestMeta {
                    request_id: Some(1),
                    opcode: 1,
                    flags: 0,
                    body_len: 5,
                }),
                body: NacelleBody::bytes(Bytes::from_static(b"tower")),
            })
            .await
            .expect("tower handler should succeed");

        let mut body = response.body;
        let chunk = body
            .next_chunk()
            .await
            .expect("response should have chunk")
            .expect("chunk should succeed");
        assert_eq!(chunk, Bytes::from_static(b"tower"));
    }
}
