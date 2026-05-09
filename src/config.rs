use std::collections::BTreeMap;

use anyhow::{Context, Result, anyhow, bail};

use crate::{
    logger,
    provider::Provider,
    user_config::UserConfig,
};

const DEFAULT_SYSTEM_PROMPT: &str = "You and the user are having a conversation. This is not a user-assistant interaction, simply a conversation between two minds.";
const DEFAULT_MAX_TOKENS: u32 = 8192;

#[derive(Clone, Debug)]
pub struct Config {
    pub provider: Provider,
    pub origin: String,
    pub api_key: Option<String>,
    pub model: String,
    pub system_prompt: String,
    pub max_tokens: u32,
    pub custom_headers: BTreeMap<String, String>,
}

impl Config {
    pub fn from_env() -> Result<Self> {
        let user_cfg = UserConfig::load()?;

        let profile_name = std::env::var("CHATTER_PROFILE")
            .ok()
            .or_else(|| user_cfg.active.clone());

        let profile = match &profile_name {
            Some(name) => {
                let p = user_cfg.profile.get(name).ok_or_else(|| {
                    anyhow!("active profile \"{name}\" is not defined in config.toml")
                })?;
                Some(p)
            }
            None => None,
        };

        let provider: Provider = if let Ok(s) = std::env::var("CHATTER_PROVIDER") {
            s.parse().context("invalid CHATTER_PROVIDER")?
        } else if let Some(p) = profile.map(|p| p.provider) {
            p
        } else {
            Provider::Anthropic
        };

        let raw_origin = std::env::var("CHATTER_ORIGIN")
            .ok()
            .or_else(|| profile.and_then(|p| p.origin.clone()))
            .unwrap_or_else(|| provider.default_origin().to_string());
        let origin = validate_origin(&raw_origin)?;

        let api_key = std::env::var("CHATTER_API_KEY")
            .ok()
            .or_else(|| profile.and_then(|p| p.api_key.clone()));

        let model = std::env::var("CHATTER_MODEL")
            .ok()
            .or_else(|| profile.and_then(|p| p.model.clone()))
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| provider.default_model().to_string());

        let system_prompt = std::env::var("CHATTER_SYSTEM_PROMPT")
            .ok()
            .or_else(|| profile.and_then(|p| p.system_prompt.clone()))
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| DEFAULT_SYSTEM_PROMPT.to_string());

        let max_tokens = std::env::var("CHATTER_MAX_TOKENS")
            .ok()
            .map(|s| s.parse::<u32>().context("invalid CHATTER_MAX_TOKENS"))
            .transpose()?
            .or_else(|| profile.and_then(|p| p.max_tokens))
            .unwrap_or(DEFAULT_MAX_TOKENS);

        let mut custom_headers: BTreeMap<String, String> = profile
            .map(|p| p.custom_headers.clone())
            .unwrap_or_default();
        if let Ok(raw) = std::env::var("CHATTER_CUSTOM_HEADERS") {
            for (k, v) in parse_custom_headers(&raw)? {
                custom_headers.insert(k, v);
            }
        }

        let has_auth = api_key.is_some()
            || custom_headers.keys().any(|k| {
                let k = k.to_lowercase();
                k == "authorization" || k == "x-api-key"
            });
        if !has_auth {
            logger::warn(format_args!(
                "no API key configured; set api_key in config.toml or CHATTER_API_KEY"
            ));
        }

        Ok(Self {
            provider,
            origin,
            api_key,
            model,
            system_prompt,
            max_tokens,
            custom_headers,
        })
    }
}

/// Accepts `scheme://host` or `scheme://host:port`. Rejects URLs with a path component.
/// Strips a trailing slash for convenience.
pub(crate) fn validate_origin(origin: &str) -> Result<String> {
    let origin = origin.trim_end_matches('/');
    let host_start = origin.find("://").map(|i| i + 3).unwrap_or(0);
    if origin[host_start..].contains('/') {
        bail!(
            "origin must be scheme://host or scheme://host:port, not a full URL with a path \
             (got: {origin}). The API path is chosen automatically per provider."
        );
    }
    Ok(origin.to_string())
}

fn parse_custom_headers(raw: &str) -> Result<Vec<(String, String)>> {
    let mut headers = Vec::new();
    for segment in raw.split(',') {
        let segment = segment.trim();
        if segment.is_empty() {
            continue;
        }
        let (name, value) = segment
            .split_once('=')
            .ok_or_else(|| anyhow!("invalid CHATTER_CUSTOM_HEADERS entry (missing '='): {segment}"))?;
        let name = name.trim().to_string();
        let value = value.trim().to_string();
        if name.is_empty() {
            return Err(anyhow!(
                "invalid CHATTER_CUSTOM_HEADERS entry (empty header name): {segment}"
            ));
        }
        headers.push((name, value));
    }
    Ok(headers)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_custom_headers_empty_string() {
        assert!(parse_custom_headers("").unwrap().is_empty());
    }

    #[test]
    fn parse_custom_headers_single() {
        assert_eq!(
            parse_custom_headers("X-Key=secret").unwrap(),
            vec![("X-Key".to_string(), "secret".to_string())]
        );
    }

    #[test]
    fn parse_custom_headers_multiple() {
        assert_eq!(
            parse_custom_headers("X-Key=abc,user=alice").unwrap(),
            vec![
                ("X-Key".to_string(), "abc".to_string()),
                ("user".to_string(), "alice".to_string()),
            ]
        );
    }

    #[test]
    fn parse_custom_headers_trims_whitespace() {
        assert_eq!(
            parse_custom_headers("  X-Key = abc , user = alice ").unwrap(),
            vec![
                ("X-Key".to_string(), "abc".to_string()),
                ("user".to_string(), "alice".to_string()),
            ]
        );
    }

    #[test]
    fn parse_custom_headers_trailing_comma() {
        assert_eq!(
            parse_custom_headers("X-Key=abc,").unwrap(),
            vec![("X-Key".to_string(), "abc".to_string())]
        );
    }

    #[test]
    fn parse_custom_headers_value_with_equals() {
        assert_eq!(
            parse_custom_headers("Authorization=Bearer token==").unwrap(),
            vec![("Authorization".to_string(), "Bearer token==".to_string())]
        );
    }

    #[test]
    fn parse_custom_headers_missing_equals_errors() {
        let err = parse_custom_headers("bogus").unwrap_err();
        assert!(err.to_string().contains("bogus"), "{err}");
    }

    #[test]
    fn parse_custom_headers_empty_name_errors() {
        let err = parse_custom_headers("=value").unwrap_err();
        assert!(err.to_string().contains("empty header name"), "{err}");
    }

    #[test]
    fn validate_origin_accepts_bare_host() {
        assert_eq!(
            validate_origin("https://api.anthropic.com").unwrap(),
            "https://api.anthropic.com"
        );
    }

    #[test]
    fn validate_origin_strips_trailing_slash() {
        assert_eq!(
            validate_origin("https://api.anthropic.com/").unwrap(),
            "https://api.anthropic.com"
        );
    }

    #[test]
    fn validate_origin_accepts_port() {
        assert_eq!(
            validate_origin("http://localhost:8080").unwrap(),
            "http://localhost:8080"
        );
    }

    #[test]
    fn validate_origin_rejects_path_component() {
        let err = validate_origin("https://api.anthropic.com/v1").unwrap_err();
        assert!(err.to_string().contains("path"), "{err}");
    }
}
