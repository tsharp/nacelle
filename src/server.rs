use std::marker::PhantomData;
use std::net::SocketAddr;
use std::sync::Arc;

use tokio::io::{AsyncRead, AsyncWrite};

use crate::config::CascadeConfig;
use crate::connection::serve_connection;
use crate::error::CascadeError;
use crate::handler::BoxedHandler;
use crate::protocol::Protocol;
use crate::registry::{HandlerRegistry, RegistryStrategy};
use crate::request::RequestMetadata;

pub struct Missing;
pub struct Present;

pub struct CascadeServer<Svc, Req, P> {
    service: Arc<Svc>,
    protocol: Arc<P>,
    registry: Arc<HandlerRegistry<Svc, Req>>,
    config: CascadeConfig,
}

impl<Svc, Req, P> Clone for CascadeServer<Svc, Req, P> {
    fn clone(&self) -> Self {
        Self {
            service: self.service.clone(),
            protocol: self.protocol.clone(),
            registry: self.registry.clone(),
            config: self.config.clone(),
        }
    }
}

impl<Svc, Req> CascadeServer<Svc, Req, ()> {
    pub fn builder() -> CascadeServerBuilder<Svc, Req, Missing, Missing, ()> {
        CascadeServerBuilder {
            service: None,
            protocol: None,
            handlers: Vec::new(),
            default_handler: None,
            config: CascadeConfig::default(),
            _service: PhantomData,
            _protocol: PhantomData,
        }
    }
}

impl<Svc, Req, P> CascadeServer<Svc, Req, P>
where
    Svc: Send + Sync + 'static,
    Req: RequestMetadata + Send + 'static,
    P: Protocol<Req> + Send + Sync + 'static,
{
    pub fn config(&self) -> &CascadeConfig {
        &self.config
    }

    pub fn service(&self) -> &Svc {
        self.service.as_ref()
    }

    pub fn protocol(&self) -> &P {
        self.protocol.as_ref()
    }

    pub fn dispatch_strategy(&self) -> RegistryStrategy {
        self.registry.strategy()
    }

    pub async fn serve_io<IO>(&self, io: IO) -> Result<(), CascadeError>
    where
        IO: AsyncRead + AsyncWrite + Unpin + Send + 'static,
    {
        serve_connection(
            io,
            self.service.clone(),
            self.protocol.clone(),
            self.registry.clone(),
            self.config.clone(),
        )
        .await
    }

    #[cfg(feature = "tcp")]
    pub async fn serve_tcp(&self, addr: SocketAddr) -> Result<(), CascadeError> {
        crate::runtime::serve_tcp(Arc::new(self.clone()), addr).await
    }
}

pub struct CascadeServerBuilder<Svc, Req, ServiceState, ProtocolState, P> {
    service: Option<Arc<Svc>>,
    protocol: Option<Arc<P>>,
    handlers: Vec<(u64, BoxedHandler<Svc, Req>)>,
    default_handler: Option<BoxedHandler<Svc, Req>>,
    config: CascadeConfig,
    _service: PhantomData<ServiceState>,
    _protocol: PhantomData<ProtocolState>,
}

impl<Svc, Req, ServiceState, ProtocolState, P>
    CascadeServerBuilder<Svc, Req, ServiceState, ProtocolState, P>
{
    pub fn config(mut self, config: CascadeConfig) -> Self {
        self.config = config;
        self
    }

    pub fn register_handler(mut self, opcode: u64, handler: BoxedHandler<Svc, Req>) -> Self {
        self.handlers.push((opcode, handler));
        self
    }

    pub fn default_handler(mut self, handler: BoxedHandler<Svc, Req>) -> Self {
        self.default_handler = Some(handler);
        self
    }
}

impl<Svc, Req, ProtocolState, P> CascadeServerBuilder<Svc, Req, Missing, ProtocolState, P> {
    pub fn service(self, service: Svc) -> CascadeServerBuilder<Svc, Req, Present, ProtocolState, P> {
        CascadeServerBuilder {
            service: Some(Arc::new(service)),
            protocol: self.protocol,
            handlers: self.handlers,
            default_handler: self.default_handler,
            config: self.config,
            _service: PhantomData,
            _protocol: PhantomData,
        }
    }
}

impl<Svc, Req, ServiceState, P> CascadeServerBuilder<Svc, Req, ServiceState, Missing, P> {
    pub fn protocol<P2>(
        self,
        protocol: P2,
    ) -> CascadeServerBuilder<Svc, Req, ServiceState, Present, P2> {
        CascadeServerBuilder {
            service: self.service,
            protocol: Some(Arc::new(protocol)),
            handlers: self.handlers,
            default_handler: self.default_handler,
            config: self.config,
            _service: PhantomData,
            _protocol: PhantomData,
        }
    }
}

impl<Svc, Req, P> CascadeServerBuilder<Svc, Req, Present, Present, P>
where
    Svc: Send + Sync + 'static,
    Req: RequestMetadata + Send + 'static,
    P: Protocol<Req> + Send + Sync + 'static,
{
    pub fn build(self) -> Result<CascadeServer<Svc, Req, P>, CascadeError> {
        let service = self.service.ok_or(CascadeError::MissingService)?;
        let protocol = self.protocol.ok_or(CascadeError::MissingProtocol)?;
        let registry = HandlerRegistry::build(self.handlers, self.default_handler)?;

        Ok(CascadeServer {
            service,
            protocol,
            registry: Arc::new(registry),
            config: self.config,
        })
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;
    use std::time::Duration;

    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    use crate::frame::{
        FRAME_FLAG_END,
        FRAME_FLAG_ERROR,
        FRAME_FLAG_START,
        FrameRequest,
        LengthDelimitedProtocol,
    };
    use crate::handler::handler_fn;
    use crate::request::{RequestBody, ResponseWriter};

    use super::*;

    #[test]
    fn build_requires_at_least_one_handler() {
        let result = CascadeServer::<(), FrameRequest, ()>::builder()
            .service(())
            .protocol(LengthDelimitedProtocol)
            .build();
        let error = match result {
            Ok(_) => panic!("builder should reject an empty registry"),
            Err(error) => error,
        };

        assert!(matches!(error, CascadeError::MissingHandler));
    }

    #[tokio::test]
    async fn streams_request_body_and_response_without_full_buffering() {
        let protocol = LengthDelimitedProtocol;
        let server = CascadeServer::<(), FrameRequest, ()>::builder()
            .service(())
            .protocol(protocol.clone())
            .config(
                CascadeConfig::default()
                    .with_request_body_chunk_size(3)
                    .with_request_body_channel_capacity(1),
            )
            .register_handler(
                7,
                handler_fn(
                    |_svc: Arc<()>,
                     _req: FrameRequest,
                     mut body: RequestBody,
                     response: ResponseWriter| async move {
                        while let Some(chunk) = body.next_chunk().await {
                            tokio::time::sleep(Duration::from_millis(5)).await;
                            response.write_bytes(chunk?)?;
                        }
                        Ok(())
                    },
                ),
            )
            .build()
            .expect("server should build");

        let (mut client, server_io) = tokio::io::duplex(256);
        let server_task = tokio::spawn(async move { server.serve_io(server_io).await });
        client
            .write_all(
                &protocol
                    .encode_request_frame(3, 7, 0, b"streaming!")
                    .expect("frame should encode"),
            )
            .await
            .expect("request should write");
        client.shutdown().await.expect("client shutdown should succeed");

        let mut payload = Vec::new();
        let mut flags = Vec::new();
        loop {
            let (_request_id, opcode, current_flags, body) =
                read_frame(&mut client).await.expect("response frame should decode");
            assert_eq!(opcode, 7);
            flags.push(current_flags);
            payload.extend_from_slice(&body);
            if current_flags & FRAME_FLAG_END != 0 {
                break;
            }
        }

        assert_eq!(payload, b"streaming!");
        assert_eq!(flags[0] & FRAME_FLAG_START, FRAME_FLAG_START);
        assert_eq!(flags.last().copied().unwrap_or_default() & FRAME_FLAG_END, FRAME_FLAG_END);
        assert!(flags.len() >= 2);
        server_task.await.expect("server task should join").expect("server should complete");
    }

    #[tokio::test]
    async fn encodes_unknown_opcode_as_error_frame() {
        let protocol = LengthDelimitedProtocol;
        let server = CascadeServer::<(), FrameRequest, ()>::builder()
            .service(())
            .protocol(protocol.clone())
            .register_handler(
                1,
                handler_fn(
                    |_svc: Arc<()>,
                     _req: FrameRequest,
                     _body: RequestBody,
                     _response: ResponseWriter| async move { Ok(()) },
                ),
            )
            .build()
            .expect("server should build");

        let (mut client, server_io) = tokio::io::duplex(256);
        let server_task = tokio::spawn(async move { server.serve_io(server_io).await });
        client
            .write_all(
                &protocol
                    .encode_request_frame(11, 99, 0, b"")
                    .expect("frame should encode"),
            )
            .await
            .expect("request should write");
        client.shutdown().await.expect("client shutdown should succeed");

        let (request_id, opcode, flags, body) =
            read_frame(&mut client).await.expect("response frame should decode");
        assert_eq!(request_id, 11);
        assert_eq!(opcode, 99);
        assert_eq!(flags & (FRAME_FLAG_START | FRAME_FLAG_END | FRAME_FLAG_ERROR), FRAME_FLAG_START | FRAME_FLAG_END | FRAME_FLAG_ERROR);
        assert!(String::from_utf8(body).expect("response must be utf8").contains("unknown opcode 99"));
        server_task.await.expect("server task should join").expect("server should complete");
    }

    async fn read_frame(
        stream: &mut tokio::io::DuplexStream,
    ) -> Result<(u64, u64, u32, Vec<u8>), CascadeError> {
        let mut len_buf = [0_u8; 4];
        stream.read_exact(&mut len_buf).await?;
        let frame_len = u32::from_le_bytes(len_buf) as usize;

        let mut frame = vec![0_u8; frame_len];
        stream.read_exact(&mut frame).await?;
        let request_id = u64::from_le_bytes(frame[0..8].try_into().expect("fixed width"));
        let opcode = u64::from_le_bytes(frame[8..16].try_into().expect("fixed width"));
        let flags = u32::from_le_bytes(frame[16..20].try_into().expect("fixed width"));
        Ok((request_id, opcode, flags, frame[20..].to_vec()))
    }
}
