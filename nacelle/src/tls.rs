use std::fs;
use std::io::{self, BufReader};
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use rustls::ServerConfig;
use rustls::pki_types::{CertificateDer, PrivateKeyDer};

#[derive(Debug, Clone)]
pub struct NacelleTlsConfig {
    server_config: Arc<ServerConfig>,
    handshake_timeout: Duration,
}

impl NacelleTlsConfig {
    pub fn from_server_config(mut server_config: ServerConfig) -> Self {
        ensure_http1_alpn(&mut server_config);
        Self {
            server_config: Arc::new(server_config),
            handshake_timeout: Duration::from_secs(10),
        }
    }

    pub fn from_server_config_arc(server_config: Arc<ServerConfig>) -> Self {
        Self {
            server_config,
            handshake_timeout: Duration::from_secs(10),
        }
    }

    pub fn from_der(
        certificates: Vec<CertificateDer<'static>>,
        private_key: PrivateKeyDer<'static>,
    ) -> io::Result<Self> {
        let config = ServerConfig::builder()
            .with_no_client_auth()
            .with_single_cert(certificates, private_key)
            .map_err(io::Error::other)?;
        Ok(Self::from_server_config(config))
    }

    pub fn from_pem(certificates: &[u8], private_key: &[u8]) -> io::Result<Self> {
        let certificates = rustls_pemfile::certs(&mut BufReader::new(certificates))
            .collect::<Result<Vec<_>, _>>()?;
        let private_key = rustls_pemfile::private_key(&mut BufReader::new(private_key))?
            .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "missing private key"))?;
        Self::from_der(certificates, private_key)
    }

    pub fn from_pem_files(
        certificate_path: impl AsRef<Path>,
        private_key_path: impl AsRef<Path>,
    ) -> io::Result<Self> {
        let certificates = fs::read(certificate_path)?;
        let private_key = fs::read(private_key_path)?;
        Self::from_pem(&certificates, &private_key)
    }

    pub fn with_handshake_timeout(mut self, timeout: Duration) -> Self {
        self.handshake_timeout = timeout;
        self
    }

    pub(crate) fn server_config(&self) -> Arc<ServerConfig> {
        self.server_config.clone()
    }

    pub(crate) fn handshake_timeout(&self) -> Duration {
        self.handshake_timeout
    }
}

fn ensure_http1_alpn(config: &mut ServerConfig) {
    if !config
        .alpn_protocols
        .iter()
        .any(|protocol| protocol == b"http/1.1")
    {
        config.alpn_protocols.push(b"http/1.1".to_vec());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tls_config_from_pem_rejects_missing_key() {
        let result = NacelleTlsConfig::from_pem(b"", b"");
        assert!(result.is_err());
    }
}
