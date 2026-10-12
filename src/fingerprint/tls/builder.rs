use crate::config::loader::get_config;
use crate::config::{http2::Http2Config, tls::TlsConfig};
use crate::fingerprint::tls::compression::{BrotliCompressor, ZlibCompressor, ZstdCompressor};
use anyhow::Result;
use btls::ssl::{AlpnError, Ssl, SslConnector, SslContextBuilder, SslMethod, SslOptions};
use btls_sys::SSL_add_application_settings;
use foreign_types_shared::ForeignType;

pub async fn tls_builder() -> Result<SslConnector> {
    let loader = get_config();
    let mut builder = SslConnector::builder(SslMethod::tls())?;
    let mut store_builder = btls::x509::store::X509StoreBuilder::new()?;

    let tls = if let Some(tls) = &loader.tls {
        tls
    } else {
        return Err(anyhow::anyhow!("TLS configuration not found"));
    };

    for cert_der in chromium_roots::TLS_SERVER_ROOT_CERTS {
        if let Ok(cert) = btls::x509::X509::from_der(cert_der) {
            store_builder.add_cert(cert)?;
        }
    }

    let store = store_builder.build();
    builder.set_cert_store(store);

    set_cipher_suites(&mut builder, &tls)?;
    set_alpn_protos(&mut builder, tls.encode_alpn_wire())?;
    set_curves_list(&mut builder, &tls)?;
    set_sigalgs_list(&mut builder, &tls)?;
    set_record_size_limit(&mut builder, &tls);
    set_cert_compression(&mut builder, &tls)?;
    set_grease(&mut builder, &tls);
    set_permute_extensions(&mut builder, &tls);
    set_status_request(&mut builder, &tls);
    set_signed_certificate_timestamp(&mut builder, &tls);
    set_session_ticket(&mut builder, &tls);
    set_delegated_credentials(&mut builder, &tls)?;
    set_extensions_order(&mut builder, &tls)?;

    let connector = builder.build();

    Ok(connector)
}
pub fn set_alpn_select_callback(builder: &mut SslContextBuilder, alpn_bytes: Vec<u8>) {
    builder.set_alpn_select_callback(move |_ssl, client_protos| {
        let mut client_cursor = client_protos;

        while !client_cursor.is_empty() {
            let len = client_cursor[0] as usize;
            if client_cursor.len() < 1 + len {
                break;
            }
            let client_proto = &client_cursor[1..1 + len];

            let mut server_cursor = &alpn_bytes[..];
            while !server_cursor.is_empty() {
                let s_len = server_cursor[0] as usize;
                let server_proto = &server_cursor[1..1 + s_len];

                if client_proto == server_proto {
                    return Ok(client_proto);
                }
                server_cursor = &server_cursor[1 + s_len..];
            }

            client_cursor = &client_cursor[1 + len..];
        }

        Err(AlpnError::NOACK)
    });
}

pub fn set_cipher_suites(builder: &mut SslContextBuilder, tls: &TlsConfig) -> Result<()> {
    builder.set_preserve_tls13_cipher_list(true);

    let ciphers = tls
        .cipher_suites
        .as_ref()
        .map(|v| v.join(":"))
        .unwrap_or_default();

    builder
        .set_cipher_list(&ciphers)
        .map_err(|e| anyhow::anyhow!("[TLS] Failed to set cipher list: {e}"))?;

    Ok(())
}

pub fn set_alpn_protos(builder: &mut SslContextBuilder, alpn_bytes: Vec<u8>) -> Result<()> {
    builder
        .set_alpn_protos(&alpn_bytes)
        .map_err(|e| anyhow::anyhow!("[TLS] Failed to set ALPN: {e}"))?;
    Ok(())
}

pub fn set_curves_list(builder: &mut SslContextBuilder, tls: &TlsConfig) -> Result<()> {
    let curve_list = tls.curves.as_ref().map(|v| v.join(":")).unwrap_or_default();
    builder
        .set_curves_list(&curve_list)
        .map_err(|e| anyhow::anyhow!("[TLS] Failed to set curves: {e}"))?;
    Ok(())
}

pub fn set_sigalgs_list(builder: &mut SslContextBuilder, tls: &TlsConfig) -> Result<()> {
    let sigalgs = tls
        .signature_algorithms
        .as_ref()
        .map(|v| v.join(":"))
        .unwrap_or_default();
    builder
        .set_sigalgs_list(&sigalgs)
        .map_err(|e| anyhow::anyhow!("[TLS] Failed to set signature algorithms: {e}"))?;
    Ok(())
}

pub fn set_extensions_order(builder: &mut SslContextBuilder, tls: &TlsConfig) -> Result<()> {
    if let Some(ext) = tls.parse_extension()? {
        builder
            .set_extension_permutation(&ext)
            .map_err(|e| anyhow::anyhow!("[TLS] Failed to set extension order: {e}"))?;
    }

    Ok(())
}

pub fn set_record_size_limit(builder: &mut SslContextBuilder, tls: &TlsConfig) {
    if let Some(limit) = tls.record_size_limit {
        builder.set_record_size_limit(limit);
    }
}

pub fn set_cert_compression(builder: &mut SslContextBuilder, tls: &TlsConfig) -> Result<()> {
    for algo in tls.cert_compression.as_deref().unwrap_or(&[]) {
        match algo.as_str() {
            "brotli" => builder
                .add_certificate_compression_algorithm(BrotliCompressor)
                .map_err(|e| anyhow::anyhow!("[TLS] Failed to add brotli compression: {e}"))?,
            "zlib" => builder
                .add_certificate_compression_algorithm(ZlibCompressor)
                .map_err(|e| anyhow::anyhow!("[TLS] Failed to add zlib compression: {e}"))?,
            "zstd" => builder
                .add_certificate_compression_algorithm(ZstdCompressor)
                .map_err(|e| anyhow::anyhow!("[TLS] Failed to add zstd compression: {e}"))?,
            other => {
                return Err(anyhow::anyhow!(
                    "[TLS] Unknown cert compression algorithm: {other}"
                ));
            }
        }
    }
    Ok(())
}

pub fn set_grease(builder: &mut SslContextBuilder, tls: &TlsConfig) {
    if let Some(grease) = &tls.grease_enabled {
        builder.set_grease_enabled(*grease);
    } else {
        builder.set_grease_enabled(false);
    }
}

pub fn set_permute_extensions(builder: &mut SslContextBuilder, tls: &TlsConfig) {
    if let Some(permute) = &tls.permute_extensions {
        builder.set_permute_extensions(*permute);
    } else {
        builder.set_permute_extensions(false);
    }
}

pub fn set_status_request(builder: &mut SslContextBuilder, tls: &TlsConfig) {
    if let Some(sts) = &tls.status_request {
        if *sts {
            builder.enable_ocsp_stapling();
        }
    }
}

pub fn set_signed_certificate_timestamp(builder: &mut SslContextBuilder, tls: &TlsConfig) {
    if let Some(sct) = &tls.signed_certificate_timestamp {
        if *sct {
            builder.enable_signed_cert_timestamps();
        }
    }
}

pub fn set_session_ticket(builder: &mut SslContextBuilder, tls: &TlsConfig) {
    if let Some(ticket) = &tls.session_ticket {
        if *ticket {
            builder.clear_options(SslOptions::NO_TICKET);
        } else {
            builder.set_options(SslOptions::NO_TICKET);
        }
    }
}

pub fn set_alps(ssl: &mut Ssl, http2: &Http2Config, tls: &TlsConfig) {
    if let Some(alps) = &tls.alps {
        if *alps {
            ssl.set_alps_use_new_codepoint(true);

            let alps_payload = http2.encode_alps_payload();
            let proto = b"h2";

            unsafe {
                let res = SSL_add_application_settings(
                    ssl.as_ptr() as *mut _,
                    proto.as_ptr(),
                    proto.len(),
                    alps_payload.as_ptr(),
                    alps_payload.len(),
                );

                if res != 1 {
                    crate::log_tag!(warn, "TLS", "Failed to set ALPS settings payload");
                }
            }
        }
    }
}

pub fn set_delegated_credentials(builder: &mut SslContextBuilder, tls: &TlsConfig) -> Result<()> {
    if let Some(algs) = &tls.delegated_credentials {
        let list = algs.join(":");
        builder
            .set_delegated_credentials(&list)
            .map_err(|e| anyhow::anyhow!("[TLS] Failed to set delegated credentials: {e}"))?;
    }
    Ok(())
}
