//! API keys. Real keys go to the Secret Service keyring; tests use memory.

use std::collections::HashMap;
use std::sync::Mutex;

pub trait SecretStore: Send + Sync {
    fn get(&self, provider_id: &str) -> Option<String>;
    fn set(&self, provider_id: &str, key: &str) -> Result<(), String>;
    fn delete(&self, provider_id: &str) -> Result<(), String>;
}

/// The desktop keyring (GNOME Keyring, KWallet via Secret Service).
pub struct Keyring;

const SERVICE: &str = "io.github.fennec.Fennec";

impl Keyring {
    fn entry(provider_id: &str) -> Result<keyring::Entry, String> {
        keyring::Entry::new(SERVICE, provider_id).map_err(|e| format!("keyring unavailable: {e}"))
    }
}

impl SecretStore for Keyring {
    fn get(&self, provider_id: &str) -> Option<String> {
        match Self::entry(provider_id).and_then(|e| e.get_password().map_err(|e| e.to_string())) {
            Ok(k) => Some(k),
            Err(e) => {
                tracing::debug!(provider_id, "no API key: {e}");
                None
            }
        }
    }

    fn set(&self, provider_id: &str, key: &str) -> Result<(), String> {
        Self::entry(provider_id)?
            .set_password(key)
            .map_err(|e| format!("could not save the key: {e}"))
    }

    fn delete(&self, provider_id: &str) -> Result<(), String> {
        match Self::entry(provider_id)?.delete_credential() {
            Ok(()) | Err(keyring::Error::NoEntry) => Ok(()),
            Err(e) => Err(format!("could not remove the key: {e}")),
        }
    }
}

#[derive(Default)]
pub struct MemorySecrets(Mutex<HashMap<String, String>>);

impl MemorySecrets {
    pub fn with(provider_id: &str, key: &str) -> Self {
        let s = Self::default();
        s.set(provider_id, key).unwrap();
        s
    }
}

impl SecretStore for MemorySecrets {
    fn get(&self, provider_id: &str) -> Option<String> {
        self.0.lock().unwrap().get(provider_id).cloned()
    }
    fn set(&self, provider_id: &str, key: &str) -> Result<(), String> {
        self.0.lock().unwrap().insert(provider_id.into(), key.into());
        Ok(())
    }
    fn delete(&self, provider_id: &str) -> Result<(), String> {
        self.0.lock().unwrap().remove(provider_id);
        Ok(())
    }
}
