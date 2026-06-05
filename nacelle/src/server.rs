use std::marker::PhantomData;
#[cfg(feature = "raw_tcp")]
use std::net::SocketAddr;
use std::sync::Arc;

use crate::config::NacelleConfig;
use crate::connection::serve_connection;
use crate::error::NacelleError;
use crate::handler::BoxedHandler;
use crate::protocol::Protocol;
use crate::request::RequestMetadata;
use tokio::io::{AsyncRead, AsyncWrite};

pub struct Missing;
pub struct Present;

pub struct NacelleServer<Svc, Req, P> {
    service: Arc<Svc>,
    protocol: Arc<P>,
    handler: BoxedHandler<Svc>,
    config: NacelleConfig,
    _request: PhantomData<fn() -> Req>,
}

pub type RawTcpServer<Svc, Req, P> = NacelleServer<Svc, Req, P>;

impl<Svc, Req, P> Clone for NacelleServer<Svc, Req, P> {
    fn clone(&self) -> Self {
        Self {
            service: self.service.clone(),
            protocol: self.protocol.clone(),
            handler: self.handler.clone(),
            config: self.config.clone(),
            _request: PhantomData,
        }
    }
}

impl<Svc, Req> NacelleServer<Svc, Req, ()> {
    pub fn builder() -> NacelleServerBuilder<Svc, Req, Missing, Missing, Missing, ()> {
        NacelleServerBuilder {
            service: None,
            protocol: None,
            handler: None,
            config: NacelleConfig::default(),
            _service: PhantomData,
            _protocol: PhantomData,
            _handler: PhantomData,
            _request: PhantomData,
        }
    }
}

impl<Svc, Req, P> NacelleServer<Svc, Req, P>
where
    Svc: Send + Sync + 'static,
    Req: RequestMetadata + Send + 'static,
    P: Protocol<Req> + Send + Sync + 'static,
{
    pub fn config(&self) -> &NacelleConfig {
        &self.config
    }

    pub fn service(&self) -> &Svc {
        self.service.as_ref()
    }

    pub fn protocol(&self) -> &P {
        self.protocol.as_ref()
    }

    pub async fn serve_halves<R, W>(&self, reader: R, writer: W) -> Result<(), NacelleError>
    where
        R: AsyncRead + Unpin + Send + 'static,
        W: AsyncWrite + Unpin + Send + 'static,
    {
        serve_connection(
            reader,
            writer,
            self.service.clone(),
            self.protocol.clone(),
            self.handler.clone(),
            self.config.clone(),
        )
        .await
    }

    /// Serve an I/O stream that implements Tokio's `AsyncRead + AsyncWrite`.
    pub async fn serve_io<IO>(&self, io: IO) -> Result<(), NacelleError>
    where
        IO: AsyncRead + AsyncWrite + Unpin + Send + 'static,
    {
        let (reader, writer) = tokio::io::split(io);
        serve_connection(
            reader,
            writer,
            self.service.clone(),
            self.protocol.clone(),
            self.handler.clone(),
            self.config.clone(),
        )
        .await
    }

    #[cfg(feature = "raw_tcp")]
    pub async fn serve_tcp(&self, addr: SocketAddr) -> Result<(), NacelleError> {
        crate::runtime::serve_tcp(Arc::<NacelleServer<Svc, Req, P>>::new(self.clone()), addr).await
    }
}

pub struct NacelleServerBuilder<Svc, Req, ServiceState, ProtocolState, HandlerState, P> {
    service: Option<Arc<Svc>>,
    protocol: Option<Arc<P>>,
    handler: Option<BoxedHandler<Svc>>,
    config: NacelleConfig,
    _service: PhantomData<ServiceState>,
    _protocol: PhantomData<ProtocolState>,
    _handler: PhantomData<HandlerState>,
    _request: PhantomData<fn() -> Req>,
}

impl<Svc, Req, ServiceState, ProtocolState, HandlerState, P>
    NacelleServerBuilder<Svc, Req, ServiceState, ProtocolState, HandlerState, P>
{
    pub fn config(mut self, config: NacelleConfig) -> Self {
        self.config = config;
        self
    }
}

impl<Svc, Req, ProtocolState, HandlerState, P>
    NacelleServerBuilder<Svc, Req, Missing, ProtocolState, HandlerState, P>
{
    pub fn service(
        self,
        service: Svc,
    ) -> NacelleServerBuilder<Svc, Req, Present, ProtocolState, HandlerState, P> {
        NacelleServerBuilder {
            service: Some(Arc::new(service)),
            protocol: self.protocol,
            handler: self.handler,
            config: self.config,
            _service: PhantomData,
            _protocol: PhantomData,
            _handler: PhantomData,
            _request: PhantomData,
        }
    }
}

impl<Svc, Req, ServiceState, HandlerState, P>
    NacelleServerBuilder<Svc, Req, ServiceState, Missing, HandlerState, P>
{
    pub fn protocol<P2>(
        self,
        protocol: P2,
    ) -> NacelleServerBuilder<Svc, Req, ServiceState, Present, HandlerState, P2> {
        NacelleServerBuilder {
            service: self.service,
            protocol: Some(Arc::new(protocol)),
            handler: self.handler,
            config: self.config,
            _service: PhantomData,
            _protocol: PhantomData,
            _handler: PhantomData,
            _request: PhantomData,
        }
    }
}

impl<Svc, Req, ServiceState, ProtocolState, P>
    NacelleServerBuilder<Svc, Req, ServiceState, ProtocolState, Missing, P>
{
    pub fn handler(
        self,
        handler: BoxedHandler<Svc>,
    ) -> NacelleServerBuilder<Svc, Req, ServiceState, ProtocolState, Present, P> {
        NacelleServerBuilder {
            service: self.service,
            protocol: self.protocol,
            handler: Some(handler),
            config: self.config,
            _service: PhantomData,
            _protocol: PhantomData,
            _handler: PhantomData,
            _request: PhantomData,
        }
    }
}

impl<Svc, Req, P> NacelleServerBuilder<Svc, Req, Present, Present, Present, P>
where
    Svc: Send + Sync + 'static,
    Req: RequestMetadata + Send + 'static,
    P: Protocol<Req> + Send + Sync + 'static,
{
    pub fn build(self) -> Result<NacelleServer<Svc, Req, P>, NacelleError> {
        let service = self.service.ok_or(NacelleError::MissingService)?;
        let protocol = self.protocol.ok_or(NacelleError::MissingProtocol)?;
        let handler = self.handler.expect("handler state guarantees a handler");

        Ok(NacelleServer {
            service,
            protocol,
            handler,
            config: self.config,
            _request: PhantomData,
        })
    }
}

#[cfg(all(test, feature = "reference_protocol"))]
mod tests {
    use std::sync::Arc;
    use std::time::Duration;

    use bytes::Bytes;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    use crate::handler::handler_fn;
    use crate::reference_protocol::{
        FRAME_FLAG_END, FRAME_FLAG_ERROR, FRAME_FLAG_START, FrameRequest, LengthDelimitedProtocol,
    };
    use crate::response::{NacelleResponse, RawTcpResponseMeta};

    use super::*;

    #[tokio::test]
    async fn streams_request_body_and_response_without_full_buffering() {
        let protocol = LengthDelimitedProtocol;
        let server = NacelleServer::<(), FrameRequest, ()>::builder()
            .service(())
            .protocol(protocol.clone())
            .config(
                NacelleConfig::default()
                    .with_request_body_chunk_size(3)
                    .with_request_body_channel_capacity(1),
            )
            .handler(handler_fn(|_svc: Arc<()>, mut request| async move {
                let mut chunks = Vec::new();
                while let Some(chunk) = request.body.next_chunk().await {
                    tokio::time::sleep(Duration::from_millis(5)).await;
                    chunks.push(chunk?);
                }
                let (tx, body) = crate::request::NacelleBody::channel(1);
                tokio::spawn(async move {
                    for chunk in chunks {
                        if tx.send(Ok(chunk)).await.is_err() {
                            break;
                        }
                    }
                });
                Ok(NacelleResponse::raw_tcp(body))
            }))
            .build()
            .expect("server should build");

        let (mut client, server_io) = tokio::io::duplex(256);
        let server_task = tokio::spawn(async move { server.serve_io(server_io).await });
        AsyncWriteExt::write_all(
            &mut client,
            &protocol
                .encode_request_frame(3, 7, 0, b"streaming!")
                .expect("frame should encode"),
        )
        .await
        .expect("request should write");
        client
            .shutdown()
            .await
            .expect("client shutdown should succeed");

        let mut payload = Vec::new();
        let mut flags = Vec::new();
        loop {
            let (_request_id, opcode, current_flags, body) = read_frame(&mut client)
                .await
                .expect("response frame should decode");
            assert_eq!(opcode, 7);
            flags.push(current_flags);
            payload.extend_from_slice(&body);
            if current_flags & FRAME_FLAG_END != 0 {
                break;
            }
        }

        assert_eq!(payload, b"streaming!");
        assert_eq!(flags[0] & FRAME_FLAG_START, FRAME_FLAG_START);
        assert_eq!(
            flags.last().copied().unwrap_or_default() & FRAME_FLAG_END,
            FRAME_FLAG_END
        );
        assert!(flags.len() >= 2);
        server_task
            .await
            .expect("server task should join")
            .expect("server should complete");
    }

    #[tokio::test]
    async fn application_router_can_encode_unknown_opcode_error() {
        let protocol = LengthDelimitedProtocol;
        let server = routed_echo_server(protocol.clone(), NacelleConfig::default());

        let (mut client, server_io) = tokio::io::duplex(256);
        let server_task = tokio::spawn(async move { server.serve_io(server_io).await });
        AsyncWriteExt::write_all(
            &mut client,
            &protocol
                .encode_request_frame(11, 99, 0, b"")
                .expect("frame should encode"),
        )
        .await
        .expect("request should write");
        client
            .shutdown()
            .await
            .expect("client shutdown should succeed");

        let (request_id, opcode, flags, body) = read_frame(&mut client)
            .await
            .expect("response frame should decode");
        assert_eq!(request_id, 11);
        assert_eq!(opcode, 99);
        assert_eq!(
            flags & (FRAME_FLAG_START | FRAME_FLAG_END | FRAME_FLAG_ERROR),
            FRAME_FLAG_START | FRAME_FLAG_END | FRAME_FLAG_ERROR
        );
        assert!(
            String::from_utf8(body)
                .expect("response must be utf8")
                .contains("unknown opcode 99")
        );
        server_task
            .await
            .expect("server task should join")
            .expect("server should complete");
    }

    #[tokio::test]
    async fn accepts_request_arriving_in_fragments() {
        let protocol = LengthDelimitedProtocol;
        let server = echo_server(protocol.clone(), NacelleConfig::default());
        let frame = protocol
            .encode_request_frame(3, 1, 0, b"fragmented")
            .expect("frame should encode");

        let (mut client, server_io) = tokio::io::duplex(256);
        let server_task = tokio::spawn(async move { server.serve_io(server_io).await });
        client
            .write_all(&frame[..7])
            .await
            .expect("first fragment should write");
        tokio::task::yield_now().await;
        client
            .write_all(&frame[7..])
            .await
            .expect("second fragment should write");
        client
            .shutdown()
            .await
            .expect("client shutdown should succeed");

        let (request_id, opcode, flags, body) = read_frame(&mut client)
            .await
            .expect("response frame should decode");
        assert_eq!(request_id, 3);
        assert_eq!(opcode, 1);
        assert_eq!(
            flags & (FRAME_FLAG_START | FRAME_FLAG_END),
            FRAME_FLAG_START | FRAME_FLAG_END
        );
        assert_eq!(body, b"fragmented");
        server_task
            .await
            .expect("server task should join")
            .expect("server should complete");
    }

    #[tokio::test]
    async fn eof_mid_head_fails_connection() {
        let protocol = LengthDelimitedProtocol;
        let server = echo_server(protocol, NacelleConfig::default());
        let (mut client, server_io) = tokio::io::duplex(256);
        let server_task = tokio::spawn(async move { server.serve_io(server_io).await });

        client
            .write_all(&[24, 0, 0])
            .await
            .expect("partial head should write");
        client
            .shutdown()
            .await
            .expect("client shutdown should succeed");

        let error = server_task
            .await
            .expect("server task should join")
            .expect_err("server should reject truncated head");
        assert!(matches!(error, NacelleError::UnexpectedEof));
    }

    #[tokio::test]
    async fn eof_mid_body_fails_connection() {
        let protocol = LengthDelimitedProtocol;
        let server = echo_server(protocol.clone(), NacelleConfig::default());
        let frame = protocol
            .encode_request_frame(3, 1, 0, b"incomplete")
            .expect("frame should encode");

        let (mut client, server_io) = tokio::io::duplex(256);
        let server_task = tokio::spawn(async move { server.serve_io(server_io).await });
        client
            .write_all(&frame[..frame.len() - 3])
            .await
            .expect("partial body should write");
        client
            .shutdown()
            .await
            .expect("client shutdown should succeed");

        let error = server_task
            .await
            .expect("server task should join")
            .expect_err("server should reject truncated body");
        assert!(matches!(error, NacelleError::UnexpectedEof));
    }

    #[tokio::test]
    async fn oversized_frame_fails_connection() {
        let protocol = LengthDelimitedProtocol;
        let server = echo_server(
            protocol.clone(),
            NacelleConfig::default().with_max_frame_len(24),
        );
        let frame = protocol
            .encode_request_frame(3, 1, 0, b"too-large")
            .expect("frame should encode");

        let (mut client, server_io) = tokio::io::duplex(256);
        let server_task = tokio::spawn(async move { server.serve_io(server_io).await });
        client
            .write_all(&frame)
            .await
            .expect("oversized frame should write");
        client
            .shutdown()
            .await
            .expect("client shutdown should succeed");

        let error = server_task
            .await
            .expect("server task should join")
            .expect_err("server should reject oversized frame");
        assert!(matches!(error, NacelleError::FrameTooLarge { .. }));
    }

    #[tokio::test]
    async fn unknown_opcode_drains_body_before_next_request() {
        let protocol = LengthDelimitedProtocol;
        let server = routed_echo_server(protocol.clone(), NacelleConfig::default());
        let unknown = protocol
            .encode_request_frame(10, 99, 0, b"body that must be drained")
            .expect("unknown frame should encode");
        let known = protocol
            .encode_request_frame(11, 1, 0, b"next")
            .expect("known frame should encode");

        let (mut client, server_io) = tokio::io::duplex(512);
        let server_task = tokio::spawn(async move { server.serve_io(server_io).await });
        client
            .write_all(&[unknown, known].concat())
            .await
            .expect("pipelined frames should write");
        client
            .shutdown()
            .await
            .expect("client shutdown should succeed");

        let (request_id, opcode, flags, body) = read_frame(&mut client)
            .await
            .expect("error frame should decode");
        assert_eq!((request_id, opcode), (10, 99));
        assert_eq!(
            flags & (FRAME_FLAG_START | FRAME_FLAG_END | FRAME_FLAG_ERROR),
            FRAME_FLAG_START | FRAME_FLAG_END | FRAME_FLAG_ERROR
        );
        assert!(
            String::from_utf8(body)
                .expect("error body should be utf8")
                .contains("unknown opcode 99")
        );

        let (request_id, opcode, flags, body) = read_frame(&mut client)
            .await
            .expect("next response should decode");
        assert_eq!((request_id, opcode), (11, 1));
        assert_eq!(
            flags & (FRAME_FLAG_START | FRAME_FLAG_END),
            FRAME_FLAG_START | FRAME_FLAG_END
        );
        assert_eq!(body, b"next");
        server_task
            .await
            .expect("server task should join")
            .expect("server should complete");
    }

    #[tokio::test]
    async fn handler_error_without_response_becomes_error_frame() {
        let protocol = LengthDelimitedProtocol;
        let server = NacelleServer::<(), FrameRequest, ()>::builder()
            .service(())
            .protocol(protocol.clone())
            .handler(handler_fn(|_svc: Arc<()>, _request| async move {
                Err(NacelleError::handler(std::io::Error::other("handler boom")))
            }))
            .build()
            .expect("server should build");

        let (mut client, server_io) = tokio::io::duplex(256);
        let server_task = tokio::spawn(async move { server.serve_io(server_io).await });
        client
            .write_all(
                &protocol
                    .encode_request_frame(42, 1, 0, b"")
                    .expect("frame should encode"),
            )
            .await
            .expect("request should write");
        client
            .shutdown()
            .await
            .expect("client shutdown should succeed");

        let (request_id, opcode, flags, body) = read_frame(&mut client)
            .await
            .expect("error response should decode");
        assert_eq!((request_id, opcode), (42, 1));
        assert_eq!(
            flags & (FRAME_FLAG_START | FRAME_FLAG_END | FRAME_FLAG_ERROR),
            FRAME_FLAG_START | FRAME_FLAG_END | FRAME_FLAG_ERROR
        );
        assert!(
            String::from_utf8(body)
                .expect("error body should be utf8")
                .contains("handler boom")
        );
        server_task
            .await
            .expect("server task should join")
            .expect("server should complete");
    }

    #[tokio::test]
    async fn multiple_response_writes_become_ordered_frames() {
        let protocol = LengthDelimitedProtocol;
        let server = NacelleServer::<(), FrameRequest, ()>::builder()
            .service(())
            .protocol(protocol.clone())
            .handler(handler_fn(|_svc: Arc<()>, _request| async move {
                let (tx, body) = crate::request::NacelleBody::channel(2);
                tx.send(Ok(Bytes::from_static(b"first")))
                    .await
                    .expect("receiver should be open");
                tx.send(Ok(Bytes::from_static(b"second")))
                    .await
                    .expect("receiver should be open");
                drop(tx);
                Ok(NacelleResponse::raw_tcp(body))
            }))
            .build()
            .expect("server should build");

        let (mut client, server_io) = tokio::io::duplex(256);
        let server_task = tokio::spawn(async move { server.serve_io(server_io).await });
        client
            .write_all(
                &protocol
                    .encode_request_frame(42, 1, 0, b"")
                    .expect("frame should encode"),
            )
            .await
            .expect("request should write");
        client
            .shutdown()
            .await
            .expect("client shutdown should succeed");

        let (_, _, flags, body) = read_frame(&mut client)
            .await
            .expect("first response should decode");
        assert_eq!(flags & FRAME_FLAG_START, FRAME_FLAG_START);
        assert_eq!(flags & FRAME_FLAG_END, 0);
        assert_eq!(body, b"first");

        let (_, _, flags, body) = read_frame(&mut client)
            .await
            .expect("second response should decode");
        assert_eq!(flags & FRAME_FLAG_START, 0);
        assert_eq!(flags & FRAME_FLAG_END, FRAME_FLAG_END);
        assert_eq!(body, b"second");
        server_task
            .await
            .expect("server task should join")
            .expect("server should complete");
    }

    #[tokio::test]
    async fn raw_tcp_response_meta_can_override_response_opcode() {
        let protocol = LengthDelimitedProtocol;
        let server = NacelleServer::<(), FrameRequest, ()>::builder()
            .service(())
            .protocol(protocol.clone())
            .handler(handler_fn(|_svc: Arc<()>, _request| async move {
                Ok(NacelleResponse::raw_tcp_with_meta(
                    RawTcpResponseMeta {
                        request_id: None,
                        opcode: Some(77),
                    },
                    crate::request::NacelleBody::bytes(Bytes::from_static(b"override")),
                ))
            }))
            .build()
            .expect("server should build");

        let (mut client, server_io) = tokio::io::duplex(256);
        let server_task = tokio::spawn(async move { server.serve_io(server_io).await });
        client
            .write_all(
                &protocol
                    .encode_request_frame(42, 1, 0, b"")
                    .expect("frame should encode"),
            )
            .await
            .expect("request should write");
        client
            .shutdown()
            .await
            .expect("client shutdown should succeed");

        let (request_id, opcode, flags, body) = read_frame(&mut client)
            .await
            .expect("response should decode");
        assert_eq!(request_id, 42);
        assert_eq!(opcode, 77);
        assert_eq!(
            flags & (FRAME_FLAG_START | FRAME_FLAG_END),
            FRAME_FLAG_START | FRAME_FLAG_END
        );
        assert_eq!(body, b"override");
        server_task
            .await
            .expect("server task should join")
            .expect("server should complete");
    }

    fn echo_server(
        protocol: LengthDelimitedProtocol,
        config: NacelleConfig,
    ) -> NacelleServer<(), FrameRequest, LengthDelimitedProtocol> {
        NacelleServer::<(), FrameRequest, ()>::builder()
            .service(())
            .protocol(protocol)
            .config(config)
            .handler(handler_fn(|_svc: Arc<()>, request| async move {
                Ok(NacelleResponse::raw_tcp(request.body))
            }))
            .build()
            .expect("server should build")
    }

    fn routed_echo_server(
        protocol: LengthDelimitedProtocol,
        config: NacelleConfig,
    ) -> NacelleServer<(), FrameRequest, LengthDelimitedProtocol> {
        NacelleServer::<(), FrameRequest, ()>::builder()
            .service(())
            .protocol(protocol)
            .config(config)
            .handler(handler_fn(|_svc: Arc<()>, mut request| async move {
                let opcode = request.raw_tcp_opcode().unwrap_or_default();
                if opcode != 1 {
                    while let Some(chunk) = request.body.next_chunk().await {
                        let _ = chunk?;
                    }
                    return Err(NacelleError::handler(std::io::Error::other(format!(
                        "unknown opcode {opcode}"
                    ))));
                }

                Ok(NacelleResponse::raw_tcp(request.body))
            }))
            .build()
            .expect("server should build")
    }

    async fn read_frame(
        stream: &mut tokio::io::DuplexStream,
    ) -> Result<(u64, u64, u32, Vec<u8>), NacelleError> {
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
