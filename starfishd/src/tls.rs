//! Serving agents over TLS.

use anyhow::{Context, bail};
use std::fs::File;
use std::io::BufReader;
use std::path::Path;
use std::sync::Arc;
use tokio_rustls::TlsAcceptor;
use tokio_rustls::rustls::{ServerConfig, pki_types::CertificateDer};

/// Builds an acceptor from a PEM certificate chain and its private key.
///
/// The key may be PKCS#8, PKCS#1 or SEC1; `rustls_pemfile` works out which from
/// the PEM label, so an EC key works as well as an RSA one.
pub fn acceptor(cert_path: &Path, key_path: &Path) -> anyhow::Result<TlsAcceptor> {
    // Other crates in this workspace pull rustls in with more than one crypto
    // provider, leaving it unable to choose. Installing one here makes the
    // choice explicit rather than a panic on the first handshake. It is process
    // wide, so a second call failing is not worth reporting.
    let _ = tokio_rustls::rustls::crypto::ring::default_provider().install_default();

    let certs = load_certs(cert_path)?;
    let key = load_key(key_path)?;

    let config = ServerConfig::builder()
        .with_no_client_auth()
        .with_single_cert(certs, key)
        .context("The certificate and private key do not go together")?;

    Ok(TlsAcceptor::from(Arc::new(config)))
}

fn load_certs(path: &Path) -> anyhow::Result<Vec<CertificateDer<'static>>> {
    let file = File::open(path)
        .with_context(|| format!("Unable to open the certificate {}", path.display()))?;

    let certs: Vec<_> = rustls_pemfile::certs(&mut BufReader::new(file))
        .collect::<Result<_, _>>()
        .with_context(|| format!("Unable to read certificates from {}", path.display()))?;

    if certs.is_empty() {
        bail!("{} holds no certificates", path.display());
    }

    Ok(certs)
}

fn load_key(
    path: &Path,
) -> anyhow::Result<tokio_rustls::rustls::pki_types::PrivateKeyDer<'static>> {
    let file = File::open(path)
        .with_context(|| format!("Unable to open the private key {}", path.display()))?;

    rustls_pemfile::private_key(&mut BufReader::new(file))
        .with_context(|| format!("Unable to read a private key from {}", path.display()))?
        .with_context(|| format!("{} holds no private key", path.display()))
}
