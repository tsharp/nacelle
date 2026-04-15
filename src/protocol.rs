use bytes::{Bytes, BytesMut};

use crate::error::NacelleError;
use crate::request::RequestMetadata;

#[derive(Debug)]
pub struct DecodedRequest<Req> {
    pub request: Req,
    pub body_len: usize,
}

pub trait Protocol<Req>: Send + Sync + 'static
where
    Req: RequestMetadata,
{
    type ResponseContext: Send + 'static;
    type ErrorContext: Send + 'static;

    fn decode_head(
        &self,
        src: &mut BytesMut,
        max_frame_len: usize,
    ) -> Result<Option<DecodedRequest<Req>>, NacelleError>;

    fn response_context(&self, req: &Req) -> Self::ResponseContext;

    fn error_context(&self, req: &Req) -> Self::ErrorContext;

    fn encode_response_chunk(
        &self,
        context: &mut Self::ResponseContext,
        chunk: Bytes,
        dst: &mut BytesMut,
    ) -> Result<(), NacelleError>;

    fn encode_response_terminal_chunk(
        &self,
        context: &mut Self::ResponseContext,
        chunk: Bytes,
        dst: &mut BytesMut,
    ) -> Result<(), NacelleError> {
        self.encode_response_chunk(context, chunk, dst)?;
        self.encode_response_end(context, dst)
    }

    fn encode_response_end(
        &self,
        context: &mut Self::ResponseContext,
        dst: &mut BytesMut,
    ) -> Result<(), NacelleError>;

    fn encode_error(
        &self,
        context: Option<&Self::ErrorContext>,
        error: &NacelleError,
        dst: &mut BytesMut,
    ) -> Result<(), NacelleError>;
}
