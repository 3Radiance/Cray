use btls::ssl::ExtensionType;
use serde::Deserialize;

#[derive(Debug, Deserialize, Clone)]
#[allow(dead_code)]
pub struct TlsConfig {
    pub cipher_suites: Option<Vec<String>>,
    pub alpn: Option<Vec<String>>,
    pub curves: Option<Vec<String>>,
    pub signature_algorithms: Option<Vec<String>>,
    pub extensions_order: Option<Vec<String>>,
    pub cert_compression: Option<Vec<String>>,
    pub permute_extensions: Option<bool>,
    pub status_request: Option<bool>,
    pub signed_certificate_timestamp: Option<bool>,
    pub alps: Option<bool>,
    pub session_ticket: Option<bool>,
    pub grease_enabled: Option<bool>,
    pub enable_ech: Option<bool>,
    pub enable_ech_grease: Option<bool>,
    pub delegated_credentials: Option<Vec<String>>,
    pub record_size_limit: Option<u16>,
    pub doh: Option<Vec<String>>,
}

impl TlsConfig {
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
                        "signed_certificate_timestamp" => Ok(ExtensionType::CERTIFICATE_TIMESTAMP),
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
}
