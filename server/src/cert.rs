use rcgen::generate_simple_self_signed;
use rustls::pki_types::{CertificateDer, PrivateKeyDer};

pub fn generate_self_signed_cert(
) -> Result<(rustls::ServerConfig, Vec<u8>), Box<dyn std::error::Error + Send + Sync>> {
    let subject_alt_names = vec!["localhost".to_string(), "127.0.0.1".to_string()];
    let cert = generate_simple_self_signed(subject_alt_names)?;

    let cert_der = cert.cert.der().to_vec();
    let key_der = cert.key_pair.serialize_der();

    let cert_chain = vec![CertificateDer::from(cert_der.clone())];
    let key = PrivateKeyDer::try_from(key_der.clone())?;

    let mut server_config = rustls::ServerConfig::builder()
        .with_no_client_auth()
        .with_single_cert(cert_chain, key)?;

    server_config.alpn_protocols = vec![b"tunnel".to_vec()];

    Ok((server_config, cert_der))
}
