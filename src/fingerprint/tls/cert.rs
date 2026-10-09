use anyhow::{Context, Result, anyhow};
use btls::pkey::{PKey, Private};
use btls::x509::X509;
use std::path::Path;

pub struct CertPair {
    pub chain: Vec<X509>,
    pub key: PKey<Private>,
}

impl CertPair {
    pub fn load(cert_path: impl AsRef<Path>, key_path: impl AsRef<Path>) -> Result<Self> {
        let cert_path = cert_path.as_ref();
        let key_path = key_path.as_ref();

        if !cert_path.exists() || !key_path.exists() {
            return Err(anyhow!(
                "Cert or Key file not found: {:?}, {:?}",
                cert_path,
                key_path
            ));
        }

        let cert_pem = std::fs::read(cert_path)
            .with_context(|| format!("Failed to read cert file: {:?}", cert_path))?;
        let key_pem = std::fs::read(key_path)
            .with_context(|| format!("Failed to read key file: {:?}", key_path))?;

        let chain =
            X509::stack_from_pem(&cert_pem).context("Failed to parse X509 cert chain from PEM")?;

        if chain.is_empty() {
            return Err(anyhow!(
                "No valid X509 certificates found in {:?}",
                cert_path
            ));
        }

        let key =
            PKey::private_key_from_pem(&key_pem).context("Failed to parse private key from PEM")?;

        crate::log_tag!(
            info,
            "CERT",
            "Successfully loaded TLS cert chain and private key"
        );

        Ok(Self { chain, key })
    }
}
