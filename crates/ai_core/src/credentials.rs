//! Unified AI credential container and zero-allocation key/endpoint resolution.

use boxxy_model_selection::OpenAiCompatibleEndpoint;
use boxxy_sys_utils::env::PROVIDER_ENV_KEY_MAP;
use std::borrow::Cow;
use std::collections::HashMap;

/// Unified credential payload carrying provider API keys, the Ollama base URL,
/// and user-configured OpenAI-compatible endpoints.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct AiCredentials {
    pub api_keys: HashMap<String, String>,
    pub ollama_url: String,
    pub openai_compatible_endpoints: Vec<OpenAiCompatibleEndpoint>,
}

impl AiCredentials {
    #[must_use]
    pub const fn new(api_keys: HashMap<String, String>, ollama_url: String) -> Self {
        Self {
            api_keys,
            ollama_url,
            openai_compatible_endpoints: Vec::new(),
        }
    }

    #[must_use]
    pub fn with_custom_endpoints(mut self, endpoints: Vec<OpenAiCompatibleEndpoint>) -> Self {
        self.openai_compatible_endpoints = endpoints;
        self
    }

    /// Resolves the trimmed API key for a built-in provider without allocating when
    /// present in `self.api_keys`.
    ///
    /// Falls back to `PROVIDER_ENV_KEY_MAP` environment variables when `self.api_keys`
    /// has no non-empty entry.
    #[must_use]
    pub(super) fn resolve_api_key<'a>(&'a self, provider_name: &str) -> Cow<'a, str> {
        if let Some(trimmed) = self
            .api_keys
            .get(provider_name)
            .map(|s| s.trim())
            .filter(|s| !s.is_empty())
        {
            return Cow::Borrowed(trimmed);
        }

        for &(env_var, mapped_provider) in PROVIDER_ENV_KEY_MAP {
            if mapped_provider == provider_name
                && let Ok(val) = std::env::var(env_var)
            {
                let trimmed = val.trim();
                if !trimmed.is_empty() {
                    return Cow::Owned(trimmed.to_owned());
                }
            }
        }

        Cow::Borrowed("")
    }

    /// Looks up a configured [`OpenAiCompatibleEndpoint`] by trimmed provider name.
    #[must_use]
    pub(super) fn find_custom_endpoint(
        &self,
        provider_name: &str,
    ) -> Option<&OpenAiCompatibleEndpoint> {
        let target = provider_name.trim();
        self.openai_compatible_endpoints
            .iter()
            .find(|ep| ep.name.trim() == target)
    }

    /// Resolves the trimmed API key for a custom `OpenAI`-compatible endpoint.
    ///
    /// Consults `self.api_keys`, then `endpoint.api_key_env`, and finally `"not-needed"`
    /// for unauthenticated local servers.
    #[must_use]
    pub(super) fn resolve_custom_api_key<'a>(
        &'a self,
        provider_name: &str,
        endpoint: Option<&OpenAiCompatibleEndpoint>,
    ) -> Cow<'a, str> {
        let target = provider_name.trim();
        if let Some(trimmed) = self
            .api_keys
            .get(provider_name)
            .or_else(|| {
                self.api_keys
                    .iter()
                    .find_map(|(k, v)| (k.trim() == target).then_some(v))
            })
            .map(|s| s.trim())
            .filter(|s| !s.is_empty())
        {
            return Cow::Borrowed(trimmed);
        }

        if let Some(env_var) = endpoint
            .and_then(|e| e.api_key_env.as_deref())
            .map(str::trim)
            .filter(|v| !v.is_empty())
            && let Ok(val) = std::env::var(env_var)
        {
            let trimmed = val.trim();
            if !trimmed.is_empty() {
                return Cow::Owned(trimmed.to_owned());
            }
        }

        Cow::Borrowed("not-needed")
    }

    /// Returns the normalized Ollama base URL if configured and non-empty.
    ///
    /// Returns `None` when empty so `OllamaConfig::new()` preserves its built-in
    /// `http://localhost:11434` default.
    #[must_use]
    pub(super) fn effective_ollama_url(&self) -> Option<&str> {
        let trimmed = self.ollama_url.trim().trim_end_matches('/');
        (!trimmed.is_empty()).then_some(trimmed)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolves_trimmed_borrowed_api_key_without_cloning() {
        let mut keys = HashMap::new();
        keys.insert("Gemini".to_string(), "  sk-test-key  ".to_string());
        let creds = AiCredentials::new(keys, String::new());

        let resolved = creds.resolve_api_key("Gemini");
        assert!(matches!(resolved, Cow::Borrowed("sk-test-key")));
    }

    #[test]
    fn empty_or_whitespace_ollama_url_falls_back_to_default() {
        let empty = AiCredentials::default();
        assert_eq!(empty.effective_ollama_url(), None);

        let spaces = AiCredentials::new(HashMap::new(), "   /  ".to_string());
        assert_eq!(spaces.effective_ollama_url(), None);

        let configured =
            AiCredentials::new(HashMap::new(), " http://127.0.0.1:11434/ ".to_string());
        assert_eq!(
            configured.effective_ollama_url(),
            Some("http://127.0.0.1:11434")
        );
    }

    #[test]
    fn resolves_custom_endpoint_and_fallback_key() {
        let mut ep = OpenAiCompatibleEndpoint::preset_local_server();
        ep.name = "Local vLLM".to_string();
        let creds = AiCredentials::default().with_custom_endpoints(vec![ep]);

        let found = creds.find_custom_endpoint("  Local vLLM ");
        assert!(found.is_some());
        assert_eq!(
            creds.resolve_custom_api_key("Local vLLM", found).as_ref(),
            "not-needed"
        );
    }
}
