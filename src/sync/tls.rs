//! Fennec's own certificate. Made the first time receiving is turned on and
//! kept in `<data>/sync/`. Phones learn its public key's hash (the pin) from
//! the pairing QR code and accept no other certificate, so the certificate
//! needs no authority behind it.

use std::io::Write;
use std::os::unix::fs::OpenOptionsExt;
use std::path::Path;
use std::sync::Arc;

use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use rcgen::PublicKeyData;
use rustls::pki_types::{CertificateDer, PrivateKeyDer, PrivatePkcs8KeyDer};
use sha2::{Digest, Sha256};

const CERT_FILE: &str = "cert.der";
const KEY_FILE: &str = "key.der";

pub struct Identity {
    pub cert_der: Vec<u8>,
    key_der: Vec<u8>,
    /// SHA-256 of the certificate's public key (SubjectPublicKeyInfo), in
    /// unpadded URL-safe base64. Phones pin this.
    pub pin: String,
}

impl Identity {
    /// Loads the certificate from `dir`, or makes one there if it has none.
    pub fn load_or_create(dir: &Path) -> Result<Self, String> {
        let (cert, key) = (dir.join(CERT_FILE), dir.join(KEY_FILE));
        if let (Ok(cert_der), Ok(key_der)) = (std::fs::read(&cert), std::fs::read(&key)) {
            match Self::from_der(cert_der, key_der) {
                Ok(id) => return Ok(id),
                // Paired phones will refuse the new certificate; they pair again.
                Err(e) => tracing::warn!("{}: {e}; making a new certificate", dir.display()),
            }
        }
        let id = Self::generate()?;
        std::fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
        std::fs::write(&cert, &id.cert_der).map_err(|e| format!("{}: {e}", cert.display()))?;
        std::fs::OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .mode(0o600)
            .open(&key)
            .and_then(|mut f| f.write_all(&id.key_der))
            .map_err(|e| format!("{}: {e}", key.display()))?;
        Ok(id)
    }

    fn generate() -> Result<Self, String> {
        let key = rcgen::KeyPair::generate().map_err(|e| e.to_string())?;
        let mut params =
            rcgen::CertificateParams::new(vec!["fennec.local".to_string()]).map_err(|e| e.to_string())?;
        params
            .distinguished_name
            .push(rcgen::DnType::CommonName, "Fennec");
        let cert = params.self_signed(&key).map_err(|e| e.to_string())?;
        Ok(Self {
            cert_der: cert.der().to_vec(),
            key_der: key.serialize_der(),
            pin: spki_pin(&key.subject_public_key_info()),
        })
    }

    fn from_der(cert_der: Vec<u8>, key_der: Vec<u8>) -> Result<Self, String> {
        let key = rcgen::KeyPair::try_from(key_der.as_slice()).map_err(|e| e.to_string())?;
        Ok(Self {
            pin: spki_pin(&key.subject_public_key_info()),
            cert_der,
            key_der,
        })
    }

    pub fn server_config(&self) -> Result<Arc<rustls::ServerConfig>, String> {
        let provider = Arc::new(rustls::crypto::ring::default_provider());
        let config = rustls::ServerConfig::builder_with_provider(provider)
            .with_safe_default_protocol_versions()
            .map_err(|e| e.to_string())?
            .with_no_client_auth()
            .with_single_cert(
                vec![CertificateDer::from(self.cert_der.clone())],
                PrivateKeyDer::Pkcs8(PrivatePkcs8KeyDer::from(self.key_der.clone())),
            )
            .map_err(|e| e.to_string())?;
        Ok(Arc::new(config))
    }
}

/// The pin for a DER-encoded SubjectPublicKeyInfo.
pub fn spki_pin(spki_der: &[u8]) -> String {
    URL_SAFE_NO_PAD.encode(Sha256::digest(spki_der))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_certificate_is_made_once_and_reloaded_with_the_same_pin() {
        let dir = tempfile::tempdir().unwrap();
        let first = Identity::load_or_create(dir.path()).unwrap();
        let again = Identity::load_or_create(dir.path()).unwrap();
        assert_eq!(first.pin, again.pin);
        assert_eq!(first.cert_der, again.cert_der);
        assert_eq!(first.pin.len(), 43, "32 bytes in unpadded base64");
        let mode = std::fs::metadata(dir.path().join(KEY_FILE))
            .unwrap()
            .permissions();
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(mode.mode() & 0o777, 0o600);
        first.server_config().unwrap();
    }

    #[test]
    fn a_damaged_key_is_replaced() {
        let dir = tempfile::tempdir().unwrap();
        let first = Identity::load_or_create(dir.path()).unwrap();
        std::fs::write(dir.path().join(KEY_FILE), b"not a key").unwrap();
        let second = Identity::load_or_create(dir.path()).unwrap();
        assert_ne!(first.pin, second.pin);
    }
}
