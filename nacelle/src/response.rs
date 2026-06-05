use bytes::Bytes;

use crate::request::NacelleBody;

#[derive(Debug, Clone, Default)]
pub enum RawTcpResponseMeta {
    #[default]
    Default,
}

#[cfg(feature = "http")]
#[derive(Debug, Clone)]
pub struct HttpResponseMeta {
    pub status: http::StatusCode,
    pub headers: http::HeaderMap,
}

#[derive(Debug, Clone)]
pub enum NacelleResponseMeta {
    RawTcp(RawTcpResponseMeta),
    #[cfg(feature = "http")]
    Http(HttpResponseMeta),
}

pub struct NacelleResponse {
    pub meta: NacelleResponseMeta,
    pub body: NacelleBody,
}

impl NacelleResponse {
    pub fn raw_tcp(body: NacelleBody) -> Self {
        Self {
            meta: NacelleResponseMeta::RawTcp(RawTcpResponseMeta::Default),
            body,
        }
    }

    pub fn raw_tcp_bytes(bytes: impl Into<Bytes>) -> Self {
        Self::raw_tcp(NacelleBody::bytes(bytes))
    }

    pub fn empty_raw_tcp() -> Self {
        Self::raw_tcp(NacelleBody::empty())
    }

    #[cfg(feature = "http")]
    pub fn http(status: http::StatusCode, headers: http::HeaderMap, body: NacelleBody) -> Self {
        Self {
            meta: NacelleResponseMeta::Http(HttpResponseMeta { status, headers }),
            body,
        }
    }

    #[cfg(feature = "http")]
    pub fn http_bytes(status: http::StatusCode, bytes: impl Into<Bytes>) -> Self {
        Self::http(status, http::HeaderMap::new(), NacelleBody::bytes(bytes))
    }
}
