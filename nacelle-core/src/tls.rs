use std::fs;
use std::io;
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use rustls::ServerConfig;
use rustls::pki_types::{CertificateDer, PrivateKeyDer};

#[cfg(feature = "tls-self-signed")]
#[derive(Debug, Clone)]
pub struct NacelleGeneratedTlsConfig {
    pub tls_config: NacelleTlsConfig,
    pub certificate_pem: String,
    pub private_key_pem: String,
}

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
        let certificates = parse_pem_certificates(certificates)?;
        let private_key = parse_pem_private_key(private_key)?;
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

    #[cfg(feature = "tls-self-signed")]
    pub fn self_signed(
        subject_alt_names: impl IntoIterator<Item = impl Into<String>>,
    ) -> io::Result<NacelleGeneratedTlsConfig> {
        let certified_key = rcgen::generate_simple_self_signed(
            subject_alt_names
                .into_iter()
                .map(Into::into)
                .collect::<Vec<_>>(),
        )
        .map_err(io::Error::other)?;
        let certificate_pem = certified_key.cert.pem();
        let private_key_pem = certified_key.signing_key.serialize_pem();
        let tls_config = Self::from_pem(certificate_pem.as_bytes(), private_key_pem.as_bytes())?;
        Ok(NacelleGeneratedTlsConfig {
            tls_config,
            certificate_pem,
            private_key_pem,
        })
    }

    pub fn with_handshake_timeout(mut self, timeout: Duration) -> Self {
        self.handshake_timeout = timeout;
        self
    }

    #[doc(hidden)]
    pub fn server_config(&self) -> Arc<ServerConfig> {
        self.server_config.clone()
    }

    #[doc(hidden)]
    pub fn handshake_timeout(&self) -> Duration {
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

#[doc(hidden)]
pub fn parse_pem_certificates(input: &[u8]) -> io::Result<Vec<CertificateDer<'static>>> {
    let certificates = parse_pem_blocks(input)?
        .into_iter()
        .filter(|block| block.tag() == "CERTIFICATE")
        .map(|block| CertificateDer::from(block.into_contents()))
        .collect::<Vec<_>>();
    if certificates.is_empty() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "missing certificate",
        ));
    }
    Ok(certificates)
}

fn parse_pem_private_key(input: &[u8]) -> io::Result<PrivateKeyDer<'static>> {
    for block in parse_pem_blocks(input)? {
        let tag = block.tag();
        match tag {
            "PRIVATE KEY" | "RSA PRIVATE KEY" | "EC PRIVATE KEY" => {
                return PrivateKeyDer::try_from(block.into_contents()).map_err(|error| {
                    io::Error::new(
                        io::ErrorKind::InvalidInput,
                        format!("invalid private key: {error}"),
                    )
                });
            }
            "ENCRYPTED PRIVATE KEY" => {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidInput,
                    "encrypted private keys are not supported",
                ));
            }
            _ => {}
        }
    }
    Err(io::Error::new(
        io::ErrorKind::InvalidInput,
        "missing private key",
    ))
}

fn parse_pem_blocks(input: &[u8]) -> io::Result<Vec<pem::Pem>> {
    pem::parse_many(input)
        .map_err(|error| io::Error::new(io::ErrorKind::InvalidInput, error.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tls_config_from_pem_rejects_missing_key() {
        let result = NacelleTlsConfig::from_pem(b"", b"");
        assert!(result.is_err());
    }

    #[cfg(feature = "tls-self-signed")]
    #[test]
    fn self_signed_config_generates_usable_pem() {
        let generated = NacelleTlsConfig::self_signed(["localhost"]).expect("self-signed config");
        assert!(generated.certificate_pem.contains("BEGIN CERTIFICATE"));
        assert!(generated.private_key_pem.contains("BEGIN PRIVATE KEY"));
        assert!(
            generated
                .tls_config
                .server_config()
                .alpn_protocols
                .contains(&b"http/1.1".to_vec())
        );
    }
}
