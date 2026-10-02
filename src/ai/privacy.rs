//! The privacy gate every AI request passes through.

use super::{Locality, ProviderConfig};

#[derive(Debug, Clone, PartialEq, thiserror::Error)]
pub enum PrivacyError {
    #[error("this project is local only; {provider} is a cloud service")]
    LocalOnly { provider: String },
    /// The user has not yet agreed to send this document to `provider`.
    #[error("sending to {provider} needs your confirmation")]
    NeedsConsent {
        provider: String,
        /// The provider's `id`, which the confirmation is recorded for.
        provider_id: String,
    },
}

/// May text from a document (or project) go to `provider`?
///
/// `local_only`: the project forbids cloud providers.
/// `consented`: the user confirmed sending this document to this provider.
pub fn check(provider: &ProviderConfig, local_only: bool, consented: bool) -> Result<(), PrivacyError> {
    if provider.locality != Locality::Cloud {
        return Ok(());
    }
    if local_only {
        return Err(PrivacyError::LocalOnly {
            provider: provider.name.clone(),
        });
    }
    if !consented {
        return Err(PrivacyError::NeedsConsent {
            provider: provider.name.clone(),
            provider_id: provider.id.clone(),
        });
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ai::presets;

    fn preset(id: &str) -> ProviderConfig {
        presets().into_iter().find(|p| p.config.id == id).unwrap().config
    }

    #[test]
    fn local_providers_always_pass() {
        assert!(check(&preset("ollama"), true, false).is_ok());
        assert!(check(&preset("network"), true, false).is_ok());
    }

    #[test]
    fn cloud_is_rejected_for_local_only_projects_even_with_consent() {
        assert!(matches!(
            check(&preset("claude"), true, true),
            Err(PrivacyError::LocalOnly { .. })
        ));
    }

    #[test]
    fn cloud_needs_consent_first() {
        assert!(matches!(
            check(&preset("chatgpt"), false, false),
            Err(PrivacyError::NeedsConsent { .. })
        ));
        assert!(check(&preset("chatgpt"), false, true).is_ok());
    }
}
