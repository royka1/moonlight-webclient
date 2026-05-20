use anyhow::{Context, Result};
use openssl::asn1::Asn1Time;
use openssl::bn::BigNum;
use openssl::hash::MessageDigest;
use openssl::pkey::PKey;
use openssl::rsa::Rsa;
use openssl::x509::extension::{
    BasicConstraints, KeyUsage, SubjectAlternativeName, SubjectKeyIdentifier,
};
use openssl::x509::{X509NameBuilder, X509};
use rustls::pki_types::{CertificateDer, PrivateKeyDer};
use std::path::Path;
use std::fs;
use tracing::{info};

use crate::config::Config;

pub struct TlsMaterial {
    pub cert_chain: Vec<CertificateDer<'static>>,
    pub key: PrivateKeyDer<'static>,
    pub cert_pem: String,
    pub key_pem: String,
}

/// Load existing cert/key or generate a self-signed certificate.
pub fn load_or_generate(config: &Config) -> Result<TlsMaterial> {
    if let (Some(cert_path), Some(key_path)) = (&config.cert, &config.key) {
        return load_from_paths(cert_path, key_path);
    }

    let data_dir = config.data_dir();
    let cert_path = data_dir.join("cert.pem");
    let key_path = data_dir.join("key.pem");

    if cert_path.exists() && key_path.exists() {
        info!("Loading existing TLS cert from {}", cert_path.display());
        return load_from_paths(&cert_path, &key_path);
    }

    info!(
        "Generating self-signed TLS certificate in {}",
        data_dir.display()
    );
    fs::create_dir_all(&data_dir)
        .with_context(|| format!("failed to create data dir: {}", data_dir.display()))?;

    let material = generate_self_signed()?;

    // Write PEM files.
    fs::write(&cert_path, &material.cert_pem)
        .with_context(|| format!("failed to write cert: {}", cert_path.display()))?;
    fs::write(&key_path, &material.key_pem)
        .with_context(|| format!("failed to write key: {}", key_path.display()))?;

    // Set restrictive permissions on the key (Unix only).
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if let Ok(meta) = fs::metadata(&key_path) {
            let mut perms = meta.permissions();
            perms.set_mode(0o600);
            let _ = fs::set_permissions(&key_path, perms);
        }
    }

    info!("Self-signed certificate written to {}", cert_path.display());
    Ok(material)
}

fn load_from_paths(cert_path: &Path, key_path: &Path) -> Result<TlsMaterial> {
    let cert_pem = fs::read_to_string(cert_path)
        .with_context(|| format!("failed to read cert: {}", cert_path.display()))?;
    let key_pem = fs::read_to_string(key_path)
        .with_context(|| format!("failed to read key: {}", key_path.display()))?;

    let cert_chain = rustls_pemfile::certs(&mut cert_pem.as_bytes())
        .collect::<std::result::Result<Vec<_>, _>>()
        .context("failed to parse cert PEM")?;
    let key = rustls_pemfile::private_key(&mut key_pem.as_bytes())
        .context("failed to parse key PEM")?
        .ok_or_else(|| anyhow::anyhow!("no private key found in {}", key_path.display()))?;

    if cert_chain.is_empty() {
        anyhow::bail!("no certificates found in {}", cert_path.display());
    }

    Ok(TlsMaterial {
        cert_chain,
        key,
        cert_pem,
        key_pem,
    })
}

/// Generate a self-signed certificate valid for localhost.
fn generate_self_signed() -> Result<TlsMaterial> {
    // Generate RSA key pair.
    let rsa = Rsa::generate(2048)?;
    let pkey = PKey::from_rsa(rsa)?;

    // Build X.509 name.
    let mut name_builder = X509NameBuilder::new()?;
    name_builder.append_entry_by_text("CN", "Moonlight Host Proxy")?;
    name_builder.append_entry_by_text("O", "Moonlight")?;
    let name = name_builder.build();

    // Build the certificate.
    let mut builder = X509::builder()?;
    builder.set_version(2)?; // X.509 v3

    // Serial number.
    let serial = {
        let mut buf = [0u8; 8];
        getrandom::getrandom(&mut buf).unwrap_or_default();
        let bn = BigNum::from_slice(&buf)?;
        openssl::asn1::Asn1Integer::from_bn(&bn)?
    };
    builder.set_serial_number(&serial)?;

    // Validity: 1 year before now to 10 years after.
    let now = Asn1Time::days_from_now(0)?;
    let ten_years = Asn1Time::days_from_now(365 * 10)?;
    builder.set_not_before(&now)?;
    builder.set_not_after(&ten_years)?;
    builder.set_issuer_name(&name)?;
    builder.set_subject_name(&name)?;
    builder.set_pubkey(&pkey)?;

    // SAN extension: localhost, 127.0.0.1, ::1
    let san = SubjectAlternativeName::new()
        .dns("localhost")
        .ip("127.0.0.1")
        .ip("::1")
        .build(&builder.x509v3_context(None, None))?;
    builder.append_extension(san)?;

    // Basic constraints: not a CA.
    let bc = BasicConstraints::new().build()?;
    builder.append_extension(bc)?;

    // Key usage: digital signature, key encipherment.
    let ku = KeyUsage::new()
        .digital_signature()
        .key_encipherment()
        .build()?;
    builder.append_extension(ku)?;

    // Subject key identifier.
    let ski = SubjectKeyIdentifier::new().build(&builder.x509v3_context(None, None))?;
    builder.append_extension(ski)?;

    // Self-sign.
    builder.sign(&pkey, MessageDigest::sha256())?;

    let cert = builder.build();

    // Serialize to PEM.
    let cert_pem = String::from_utf8(cert.to_pem()?)?;
    let key_pem = String::from_utf8(pkey.private_key_to_pem_pkcs8()?)?;

    // Parse into rustls types.
    let cert_chain = rustls_pemfile::certs(&mut cert_pem.as_bytes())
        .collect::<std::result::Result<Vec<_>, _>>()
        .context("failed to parse generated cert PEM")?;
    let key = rustls_pemfile::private_key(&mut key_pem.as_bytes())
        .context("failed to parse generated key PEM")?
        .ok_or_else(|| anyhow::anyhow!("no private key in generated PEM"))?;

    Ok(TlsMaterial {
        cert_chain,
        key,
        cert_pem,
        key_pem,
    })
}
