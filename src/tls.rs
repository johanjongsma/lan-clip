//! Mutual TLS pinned to lan-mouse's authorized fingerprints. Both sides
//! present their lan-mouse certificate; a peer is accepted only if its
//! certificate's fingerprint is in this machine's lan-mouse config.

use crate::lanmouse;
use anyhow::Result;
use rustls::client::danger::{HandshakeSignatureValid, ServerCertVerified, ServerCertVerifier};
use rustls::crypto::{ring, verify_tls12_signature, verify_tls13_signature, CryptoProvider};
use rustls::pki_types::{CertificateDer, ServerName, UnixTime};
use rustls::server::danger::{ClientCertVerified, ClientCertVerifier};
use rustls::{DigitallySignedStruct, DistinguishedName, Error, SignatureScheme};
use std::sync::Arc;

#[derive(Debug)]
struct Pinned(Arc<CryptoProvider>);

impl Pinned {
    fn check(&self, cert: &CertificateDer<'_>) -> Result<(), Error> {
        let fp = lanmouse::fingerprint(cert);
        if lanmouse::authorized().contains_key(&fp) {
            Ok(())
        } else {
            Err(Error::General(format!(
                "certificate {fp} is not authorized in lan-mouse's config"
            )))
        }
    }

    fn tls12(&self, m: &[u8], c: &CertificateDer<'_>, d: &DigitallySignedStruct) -> Result<HandshakeSignatureValid, Error> {
        verify_tls12_signature(m, c, d, &self.0.signature_verification_algorithms)
    }

    fn tls13(&self, m: &[u8], c: &CertificateDer<'_>, d: &DigitallySignedStruct) -> Result<HandshakeSignatureValid, Error> {
        verify_tls13_signature(m, c, d, &self.0.signature_verification_algorithms)
    }

    fn schemes(&self) -> Vec<SignatureScheme> {
        self.0.signature_verification_algorithms.supported_schemes()
    }
}

impl ServerCertVerifier for Pinned {
    fn verify_server_cert(&self, cert: &CertificateDer<'_>, _: &[CertificateDer<'_>], _: &ServerName<'_>, _: &[u8], _: UnixTime) -> Result<ServerCertVerified, Error> {
        self.check(cert).map(|_| ServerCertVerified::assertion())
    }
    fn verify_tls12_signature(&self, m: &[u8], c: &CertificateDer<'_>, d: &DigitallySignedStruct) -> Result<HandshakeSignatureValid, Error> {
        self.tls12(m, c, d)
    }
    fn verify_tls13_signature(&self, m: &[u8], c: &CertificateDer<'_>, d: &DigitallySignedStruct) -> Result<HandshakeSignatureValid, Error> {
        self.tls13(m, c, d)
    }
    fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
        self.schemes()
    }
}

impl ClientCertVerifier for Pinned {
    fn root_hint_subjects(&self) -> &[DistinguishedName] {
        &[]
    }
    fn verify_client_cert(&self, cert: &CertificateDer<'_>, _: &[CertificateDer<'_>], _: UnixTime) -> Result<ClientCertVerified, Error> {
        self.check(cert).map(|_| ClientCertVerified::assertion())
    }
    fn verify_tls12_signature(&self, m: &[u8], c: &CertificateDer<'_>, d: &DigitallySignedStruct) -> Result<HandshakeSignatureValid, Error> {
        self.tls12(m, c, d)
    }
    fn verify_tls13_signature(&self, m: &[u8], c: &CertificateDer<'_>, d: &DigitallySignedStruct) -> Result<HandshakeSignatureValid, Error> {
        self.tls13(m, c, d)
    }
    fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
        self.schemes()
    }
}

pub fn configs() -> Result<(Arc<rustls::ClientConfig>, Arc<rustls::ServerConfig>)> {
    let provider = Arc::new(ring::default_provider());
    let id = lanmouse::identity()?;
    let pinned = Arc::new(Pinned(provider.clone()));

    let client = rustls::ClientConfig::builder_with_provider(provider.clone())
        .with_safe_default_protocol_versions()?
        .dangerous()
        .with_custom_certificate_verifier(pinned.clone())
        .with_client_auth_cert(vec![id.cert.clone()], id.key.clone_key())?;

    let server = rustls::ServerConfig::builder_with_provider(provider)
        .with_safe_default_protocol_versions()?
        .with_client_cert_verifier(pinned)
        .with_single_cert(vec![id.cert], id.key)?;

    Ok((Arc::new(client), Arc::new(server)))
}

/// The certificate the other side presented, as a lan-mouse fingerprint.
pub fn peer_fingerprint(certs: Option<&[CertificateDer<'_>]>) -> Option<String> {
    certs.and_then(|c| c.first()).map(|c| lanmouse::fingerprint(c))
}
