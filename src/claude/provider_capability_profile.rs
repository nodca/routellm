use serde_json::{Map, Value};

use crate::claude::semantic_core::{ClaudeRequestExtensions, ClaudeThinkingConfig};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ResponsesCapabilityProfileKind {
    Strict,
    Compat,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AssistantHistoryDisposition {
    PreserveNativeRole,
    RetryWithTranscriptCompat,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CapabilityDisposition {
    Forward,
    Omit,
    IgnoreRequested,
    Unsupported,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClaudeProviderCapabilityProfile {
    pub provider_name: &'static str,
    pub profile_name: &'static str,
    pub profile_kind: ResponsesCapabilityProfileKind,
    pub metadata: CapabilityDisposition,
    pub service_tier: CapabilityDisposition,
    pub thinking: CapabilityDisposition,
    pub context_management: CapabilityDisposition,
    pub beta_hints: CapabilityDisposition,
    pub assistant_history: AssistantHistoryDisposition,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ClaudeExtensionResolution {
    pub metadata: ValueCapabilityDecision<Map<String, Value>>,
    pub service_tier: ValueCapabilityDecision<String>,
    pub thinking: FlagCapabilityDecision,
    pub context_management: FlagCapabilityDecision,
    pub requested_betas: Vec<String>,
    pub unsupported_beta_hints: Vec<String>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ValueCapabilityDecision<T> {
    pub disposition: CapabilityDisposition,
    pub requested: bool,
    pub forwarded: Option<T>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct FlagCapabilityDecision {
    pub disposition: CapabilityDisposition,
    pub requested: bool,
}

impl ClaudeProviderCapabilityProfile {
    pub fn responses() -> Self {
        Self::responses_strict()
    }

    pub fn responses_strict() -> Self {
        Self {
            provider_name: "responses",
            profile_name: "strict-responses",
            profile_kind: ResponsesCapabilityProfileKind::Strict,
            metadata: CapabilityDisposition::Forward,
            service_tier: CapabilityDisposition::Forward,
            thinking: CapabilityDisposition::IgnoreRequested,
            context_management: CapabilityDisposition::IgnoreRequested,
            beta_hints: CapabilityDisposition::Unsupported,
            assistant_history: AssistantHistoryDisposition::PreserveNativeRole,
        }
    }

    pub fn responses_compat() -> Self {
        Self {
            provider_name: "responses",
            profile_name: "compat-responses",
            profile_kind: ResponsesCapabilityProfileKind::Compat,
            metadata: CapabilityDisposition::Forward,
            service_tier: CapabilityDisposition::Forward,
            thinking: CapabilityDisposition::IgnoreRequested,
            context_management: CapabilityDisposition::IgnoreRequested,
            beta_hints: CapabilityDisposition::Unsupported,
            assistant_history: AssistantHistoryDisposition::RetryWithTranscriptCompat,
        }
    }

    pub fn for_responses_endpoint(base_url: &str) -> Self {
        if is_official_openai_responses_endpoint(base_url) {
            Self::responses_strict()
        } else {
            Self::responses_compat()
        }
    }

    pub fn supports_assistant_history_compat_retry(&self) -> bool {
        matches!(
            self.assistant_history,
            AssistantHistoryDisposition::RetryWithTranscriptCompat
        )
    }

    pub fn resolve_extensions(
        &self,
        extensions: &ClaudeRequestExtensions,
    ) -> ClaudeExtensionResolution {
        let requested_betas = extensions.beta_hints.values.clone();
        let unsupported_beta_hints = match self.beta_hints {
            CapabilityDisposition::Unsupported => requested_betas.clone(),
            _ => Vec::new(),
        };

        ClaudeExtensionResolution {
            metadata: ValueCapabilityDecision {
                disposition: self.metadata,
                requested: extensions.metadata.is_some(),
                forwarded: forwarded_value(self.metadata, extensions.metadata.clone()),
            },
            service_tier: ValueCapabilityDecision {
                disposition: self.service_tier,
                requested: extensions.request_hints.service_tier.is_some(),
                forwarded: forwarded_value(
                    self.service_tier,
                    extensions.request_hints.service_tier.clone(),
                ),
            },
            thinking: FlagCapabilityDecision {
                disposition: self.thinking,
                requested: extensions
                    .thinking
                    .as_ref()
                    .is_some_and(ClaudeThinkingConfig::is_requested),
            },
            context_management: FlagCapabilityDecision {
                disposition: self.context_management,
                requested: extensions.context_management.is_some(),
            },
            requested_betas,
            unsupported_beta_hints,
        }
    }
}

fn is_official_openai_responses_endpoint(base_url: &str) -> bool {
    let trimmed = base_url.trim().trim_end_matches('/').to_ascii_lowercase();
    trimmed.starts_with("https://api.openai.com") || trimmed.starts_with("http://api.openai.com")
}

fn forwarded_value<T>(disposition: CapabilityDisposition, value: Option<T>) -> Option<T> {
    match disposition {
        CapabilityDisposition::Forward => value,
        CapabilityDisposition::Omit
        | CapabilityDisposition::IgnoreRequested
        | CapabilityDisposition::Unsupported => None,
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;
    use crate::claude::semantic_core::ClaudeMessageRequest;

    #[test]
    fn claude_provider_capability_profile_responses_decides_extension_matrix() {
        let request = ClaudeMessageRequest::parse_json(&json!({
            "model": "claude-sonnet-4-6",
            "messages": [{ "role": "user", "content": "hello" }],
            "metadata": { "tenant": "ops" },
            "request_hints": { "service_tier": "priority" },
            "thinking": { "type": "enabled", "budget_tokens": 64 },
            "context_management": { "type": "ephemeral" },
            "betas": ["claude-code-20250219", "context-management-2025-06-27"]
        }))
        .expect("request should parse");

        let profile = ClaudeProviderCapabilityProfile::responses();
        let resolution = profile.resolve_extensions(&request.extensions);

        assert_eq!(profile.profile_name, "strict-responses");
        assert_eq!(
            resolution.metadata.disposition,
            CapabilityDisposition::Forward
        );
        assert_eq!(
            resolution
                .metadata
                .forwarded
                .as_ref()
                .and_then(|value| value.get("tenant")),
            Some(&json!("ops"))
        );
        assert_eq!(
            resolution.service_tier.forwarded.as_deref(),
            Some("priority")
        );
        assert_eq!(
            resolution.thinking.disposition,
            CapabilityDisposition::IgnoreRequested
        );
        assert!(resolution.thinking.requested);
        assert_eq!(
            resolution.context_management.disposition,
            CapabilityDisposition::IgnoreRequested
        );
        assert!(resolution.context_management.requested);
        assert_eq!(
            resolution.unsupported_beta_hints,
            vec![
                "claude-code-20250219".to_string(),
                "context-management-2025-06-27".to_string()
            ]
        );
        assert!(!profile.supports_assistant_history_compat_retry());
    }

    #[test]
    fn claude_provider_capability_profile_handles_unrequested_extensions() {
        let request = ClaudeMessageRequest::parse_json(&json!({
            "model": "claude-sonnet-4-6",
            "messages": [{ "role": "user", "content": "hello" }]
        }))
        .expect("request should parse");

        let resolution = ClaudeProviderCapabilityProfile::responses_strict()
            .resolve_extensions(&request.extensions);

        assert!(!resolution.metadata.requested);
        assert!(resolution.metadata.forwarded.is_none());
        assert!(!resolution.service_tier.requested);
        assert!(resolution.service_tier.forwarded.is_none());
        assert!(!resolution.thinking.requested);
        assert!(!resolution.context_management.requested);
        assert!(resolution.unsupported_beta_hints.is_empty());
    }

    #[test]
    fn claude_provider_capability_profile_classifies_official_vs_compat_responses_endpoints() {
        let strict =
            ClaudeProviderCapabilityProfile::for_responses_endpoint("https://api.openai.com/v1");
        let compat =
            ClaudeProviderCapabilityProfile::for_responses_endpoint("https://free.9e.nz/v1");

        assert_eq!(strict.profile_kind, ResponsesCapabilityProfileKind::Strict);
        assert_eq!(strict.profile_name, "strict-responses");
        assert!(!strict.supports_assistant_history_compat_retry());

        assert_eq!(compat.profile_kind, ResponsesCapabilityProfileKind::Compat);
        assert_eq!(compat.profile_name, "compat-responses");
        assert!(compat.supports_assistant_history_compat_retry());
    }
}
