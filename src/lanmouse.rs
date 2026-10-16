//! Reads lan-mouse's own config and identity, so lan-clip trusts exactly the
//! machines lan-mouse trusts and needs no pairing of its own.

use anyhow::{anyhow, Context, Result};
use base64::Engine;
use rustls::pki_types::{CertificateDer, PrivateKeyDer, PrivatePkcs8KeyDer};
use serde::Deserialize;
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::path::PathBuf;

#[derive(Debug, Clone)]
pub struct Peer {
    pub hostname: String,
    pub ips: Vec<String>,
}

#[derive(Deserialize, Default)]
struct RawConfig {
    #[serde(default)]
    authorized_fingerprints: HashMap<String, String>,
    #[serde(default)]
    clients: Vec<RawClient>,
}

#[derive(Deserialize)]
struct RawClient {
    hostname: Option<String>,
    #[serde(default)]
    ips: Vec<String>,
}

pub fn dir() -> PathBuf {
    if let Ok(d) = std::env::var("LAN_MOUSE_CONFIG_DIR") {
        return PathBuf::from(d);
    }
    let base = std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| home().join(".config"));
    base.join("lan-mouse")
}

pub fn home() -> PathBuf {
    std::env::var_os("HOME").map(PathBuf::from).unwrap_or_else(|| "/".into())
}

fn read_config() -> Result<RawConfig> {
    let path = dir().join("config.toml");
    let text = std::fs::read_to_string(&path).with_context(|| format!("reading {}", path.display()))?;
    toml::from_str(&text).with_context(|| format!("parsing {}", path.display()))
}

/// Machines lan-mouse is set up to talk to.
pub fn peers() -> Result<Vec<Peer>> {
    Ok(read_config()?
        .clients
        .into_iter()
        .filter_map(|c| {
            let hostname = c.hostname.unwrap_or_default();
            if hostname.is_empty() && c.ips.is_empty() {
                return None;
            }
            Some(Peer { hostname, ips: c.ips })
        })
        .collect())
}

/// Fingerprint -> name for every certificate lan-mouse has authorized.
/// Re-read on every connection so new lan-mouse pairings apply immediately.
pub fn authorized() -> HashMap<String, String> {
    read_config()
        .map(|c| {
            c.authorized_fingerprints
                .into_iter()
                .map(|(fp, name)| (fp.to_ascii_lowercase(), name))
                .collect()
        })
        .unwrap_or_default()
}

/// lan-mouse's format: SHA-256 of the DER certificate, lowercase hex, colon separated.
pub fn fingerprint(cert: &[u8]) -> String {
    Sha256::digest(cert).iter().map(|b| format!("{b:02x}")).collect::<Vec<_>>().join(":")
}

pub struct Identity {
    pub cert: CertificateDer<'static>,
    pub key: PrivateKeyDer<'static>,
}

/// lan-mouse writes its key with a non-standard `PRIVATE_KEY` label and CRLF
/// line endings, so the PEM is parsed by hand rather than with a PEM crate.
pub fn identity() -> Result<Identity> {
    let path = dir().join("lan-mouse.pem");
    let text = std::fs::read_to_string(&path)
        .with_context(|| format!("reading {} (has lan-mouse run on this machine?)", path.display()))?;
    let mut cert = None;
    let mut key = None;
    let mut label: Option<String> = None;
    let mut body = String::new();
    for line in text.lines().map(str::trim) {
        if let Some(l) = line.strip_prefix("-----BEGIN ").and_then(|l| l.strip_suffix("-----")) {
            label = Some(l.to_string());
            body.clear();
        } else if line.starts_with("-----END ") {
            let der = base64::engine::general_purpose::STANDARD.decode(&body)?;
            match label.take().as_deref() {
                Some("CERTIFICATE") => cert = Some(CertificateDer::from(der)),
                Some("PRIVATE_KEY") | Some("PRIVATE KEY") => {
                    key = Some(PrivateKeyDer::Pkcs8(PrivatePkcs8KeyDer::from(der)))
                }
                _ => {}
            }
        } else if label.is_some() {
            body.push_str(line);
        }
    }
    Ok(Identity {
        cert: cert.ok_or_else(|| anyhow!("no certificate in {}", path.display()))?,
        key: key.ok_or_else(|| anyhow!("no private key in {}", path.display()))?,
    })
}
