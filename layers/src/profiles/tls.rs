use anyhow::{Context, Result};
use btls::ssl::{ExtensionType, KeyShare, SslVersion};
use serde::{Deserialize, Deserializer};

#[derive(Debug, Deserialize, Clone)]
#[serde(deny_unknown_fields)]
#[allow(dead_code)]
pub struct TlsConfig {
    #[serde(deserialize_with = "deserialize_lines", default)]
    pub cipher_suites: Option<Vec<String>>,
    pub alpn: Option<Vec<String>>,
    #[serde(deserialize_with = "deserialize_lines", default)]
    pub curves: Option<Vec<String>>,
    #[serde(deserialize_with = "deserialize_lines", default)]
    pub key_shares: Option<Vec<String>>,
    #[serde(deserialize_with = "deserialize_lines", default)]
    pub signature_algorithms: Option<Vec<String>>,
    #[serde(deserialize_with = "deserialize_lines", default)]
    pub extensions_order: Option<Vec<String>>,
    pub cert_compression: Option<Vec<String>>,
    pub permute_extensions: Option<bool>,
    pub status_request: Option<bool>,
    pub signed_certificate_timestamp: Option<bool>,
    pub alps: Option<bool>,
    pub alps_use_new_codepoint: Option<bool>,
    pub session_ticket: Option<bool>,
    pub grease_enabled: Option<bool>,
    pub enable_ech: Option<bool>,
    pub enable_ech_grease: Option<bool>,
    pub aes_hw_override: Option<bool>,
    pub min_tls_version: Option<String>,
    pub max_tls_version: Option<String>,
    pub trust_anchors: Option<String>,
    #[serde(deserialize_with = "deserialize_lines", default)]
    pub delegated_credentials: Option<Vec<String>>,
    pub record_size_limit: Option<u16>,
    pub doh: Option<Vec<String>>,
}

impl TlsConfig {
    pub fn load(path: &str) -> Result<Self> {
        let contents = std::fs::read_to_string(path)
            .with_context(|| format!("cannot read tls profile '{path}'"))?;
        let config: TlsConfig =
            toml::from_str(&contents).with_context(|| format!("invalid tls profile '{path}'"))?;
        Ok(config)
    }

    pub fn encode_alpn_wire(&self) -> Vec<u8> {
        if let Some(alpn) = &self.alpn {
            let total_len: usize = alpn.iter().map(|p| 1 + p.len()).sum();
            let mut bytes = Vec::with_capacity(total_len);

            for proto in alpn {
                let len = proto.len();
                if len > 0 && len <= 255 {
                    bytes.push(len as u8);
                    bytes.extend_from_slice(proto.as_bytes());
                }
            }
            bytes
        } else {
            Vec::new()
        }
    }

    pub fn parse_extension(&self) -> anyhow::Result<Option<Vec<ExtensionType>>> {
        self.extensions_order
            .as_ref()
            .map(|extensions| {
                extensions
                    .iter()
                    .map(|s| match s.as_str() {
                        "server_name" => Ok(ExtensionType::SERVER_NAME),
                        "status_request" => Ok(ExtensionType::STATUS_REQUEST),
                        "supported_groups" => Ok(ExtensionType::SUPPORTED_GROUPS),
                        "ec_point_formats" => Ok(ExtensionType::EC_POINT_FORMATS),
                        "signature_algorithms" => Ok(ExtensionType::SIGNATURE_ALGORITHMS),
                        "alpn" => Ok(ExtensionType::APPLICATION_LAYER_PROTOCOL_NEGOTIATION),
                        "padding" => Ok(ExtensionType::PADDING),
                        "signed_certificate_timestamp" | "certificate_timestamp" => {
                            Ok(ExtensionType::CERTIFICATE_TIMESTAMP)
                        }
                        "extended_master_secret" => Ok(ExtensionType::EXTENDED_MASTER_SECRET),
                        "session_ticket" => Ok(ExtensionType::SESSION_TICKET),
                        "supported_versions" => Ok(ExtensionType::SUPPORTED_VERSIONS),
                        "psk_key_exchange_modes" => Ok(ExtensionType::PSK_KEY_EXCHANGE_MODES),
                        "signature_algorithms_cert" => Ok(ExtensionType::SIGNATURE_ALGORITHMS_CERT),
                        "key_share" => Ok(ExtensionType::KEY_SHARE),
                        "renegotiation_info" => Ok(ExtensionType::RENEGOTIATE),
                        "delegated_credentials" => Ok(ExtensionType::DELEGATED_CREDENTIAL),
                        "application_settings" => Ok(ExtensionType::APPLICATION_SETTINGS),
                        "application_settings_old" => Ok(ExtensionType::APPLICATION_SETTINGS_OLD),
                        "encrypted_client_hello" | "ech" => {
                            Ok(ExtensionType::ENCRYPTED_CLIENT_HELLO)
                        }
                        "record_size_limit" => Ok(ExtensionType::RECORD_SIZE_LIMIT),
                        "cert_compression" => Ok(ExtensionType::CERT_COMPRESSION),
                        "pre_shared_key" => Ok(ExtensionType::PRE_SHARED_KEY),
                        "early_data" => Ok(ExtensionType::EARLY_DATA),
                        "cookie" => Ok(ExtensionType::COOKIE),
                        "certificate_authorities" => Ok(ExtensionType::CERTIFICATE_AUTHORITIES),
                        "next_proto_neg" | "npn" => Ok(ExtensionType::NEXT_PROTO_NEG),
                        "channel_id" => Ok(ExtensionType::CHANNEL_ID),
                        "quic_transport_parameters_legacy" => {
                            Ok(ExtensionType::QUIC_TRANSPORT_PARAMETERS_LEGACY)
                        }
                        "quic_transport_parameters" | "quic_transport_parameters_standard" => {
                            Ok(ExtensionType::QUIC_TRANSPORT_PARAMETERS_STANDARD)
                        }
                        _ => Err(anyhow::anyhow!("unknown extension: {s}")),
                    })
                    .collect()
            })
            .transpose()
    }

    pub fn parse_key_shares(&self) -> anyhow::Result<Option<Vec<KeyShare>>> {
        self.key_shares
            .as_ref()
            .map(|shares| {
                shares
                    .iter()
                    .map(|s| match s.as_str() {
                        "X25519" => Ok(KeyShare::X25519),
                        "X25519MLKEM768" => Ok(KeyShare::X25519_MLKEM768),
                        "X25519Kyber768Draft00" => Ok(KeyShare::X25519_KYBER768_DRAFT00),
                        "P256Kyber768Draft00" => Ok(KeyShare::P256_KYBER768_DRAFT00),
                        "P-256" => Ok(KeyShare::P256),
                        "P-384" => Ok(KeyShare::P384),
                        "P-521" => Ok(KeyShare::P521),
                        "MLKEM1024" => Ok(KeyShare::MLKEM1024),
                        "ffdhe2048" => Ok(KeyShare::FFDHE2048),
                        "ffdhe3072" => Ok(KeyShare::FFDHE3072),
                        _ => Err(anyhow::anyhow!("unknown key share: {s}")),
                    })
                    .collect()
            })
            .transpose()
    }

    pub fn parse_min_tls_version(&self) -> anyhow::Result<Option<SslVersion>> {
        parse_tls_version("min_tls_version", self.min_tls_version.as_deref())
    }

    pub fn parse_max_tls_version(&self) -> anyhow::Result<Option<SslVersion>> {
        parse_tls_version("max_tls_version", self.max_tls_version.as_deref())
    }
}

fn parse_tls_version(field: &str, v: Option<&str>) -> anyhow::Result<Option<SslVersion>> {
    let Some(v) = v else {
        return Ok(None);
    };
    let version = match v.to_ascii_lowercase().as_str() {
        "tls1" | "tls1.0" => SslVersion::TLS1,
        "tls1.1" => SslVersion::TLS1_1,
        "tls1.2" => SslVersion::TLS1_2,
        "tls1.3" => SslVersion::TLS1_3,
        _ => anyhow::bail!("invalid {field}: '{v}' (expected tls1.0, tls1.1, tls1.2 or tls1.3)"),
    };
    Ok(Some(version))
}

fn deserialize_lines<'de, D>(deserializer: D) -> Result<Option<Vec<String>>, D::Error>
where
    D: Deserializer<'de>,
{
    #[derive(Deserialize)]
    #[serde(untagged)]
    enum Lines {
        One(String),
        Many(Vec<String>),
    }

    let Some(v) = Option::<Lines>::deserialize(deserializer)? else {
        return Ok(None);
    };
    let raw: Vec<String> = match v {
        Lines::One(s) => s.lines().map(str::to_owned).collect(),
        Lines::Many(v) => v,
    };
    Ok(Some(
        raw.iter()
            .map(|l| l.split('#').next().unwrap_or("").trim().to_string())
            .filter(|l| !l.is_empty())
            .collect(),
    ))
}
