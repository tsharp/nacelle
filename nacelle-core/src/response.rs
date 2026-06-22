use bytes::Bytes;

use crate::request::NacelleBody;

#[derive(Debug, Clone, Default)]
pub struct TcpResponseMeta {
    pub request_id: Option<u64>,
    pub opcode: Option<u64>,
}

#[cfg(feature = "http-types")]
#[derive(Debug, Clone)]
pub struct HttpResponseMeta {
    pub status: http::StatusCode,
    pub headers: http::HeaderMap,
}

#[derive(Debug, Clone)]
pub enum NacelleResponseMeta {
    Tcp(TcpResponseMeta),
    #[cfg(feature = "http-types")]
    Http(HttpResponseMeta),
}

impl NacelleResponseMeta {
    pub fn tcp(&self) -> Option<&TcpResponseMeta> {
        match self {
            Self::Tcp(meta) => Some(meta),
            #[cfg(feature = "http-types")]
            Self::Http(_) => None,
        }
    }
}

pub struct NacelleResponse {
    pub meta: NacelleResponseMeta,
    pub body: NacelleBody,
}

impl NacelleResponse {
    pub fn tcp(body: NacelleBody) -> Self {
        Self {
            meta: NacelleResponseMeta::Tcp(TcpResponseMeta::default()),
            body,
        }
    }

    pub fn tcp_with_meta(meta: TcpResponseMeta, body: NacelleBody) -> Self {
        Self {
            meta: NacelleResponseMeta::Tcp(meta),
            body,
        }
    }

    pub fn tcp_bytes(bytes: impl Into<Bytes>) -> Self {
        Self::tcp(NacelleBody::bytes(bytes))
    }

    pub fn empty_tcp() -> Self {
        Self::tcp(NacelleBody::empty())
    }

    #[cfg(feature = "http-types")]
    pub fn http(status: http::StatusCode, headers: http::HeaderMap, body: NacelleBody) -> Self {
        Self {
            meta: NacelleResponseMeta::Http(HttpResponseMeta { status, headers }),
            body,
        }
    }

    #[cfg(feature = "http-types")]
    pub fn http_bytes(status: http::StatusCode, bytes: impl Into<Bytes>) -> Self {
        Self::http(status, http::HeaderMap::new(), NacelleBody::bytes(bytes))
    }
}
