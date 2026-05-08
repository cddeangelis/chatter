use anyhow::{Context, Result, anyhow};

use crate::api::{ApiConfig, DEFAULT_MODEL};
use crate::user_config::UserConfig;

pub struct Config {
    pub base_url: String,
    pub custom_headers: Vec<(String, String)>,
    pub model: String,
}

impl Config {
    pub fn from_env() -> Result<Self> {
        let base_url = std::env::var("CHATTER_BASE_URL")
            .context("CHATTER_BASE_URL environment variable is not set")?;
        let base_url = base_url.trim_end_matches('/').to_string();

        let custom_headers = match std::env::var("CHATTER_CUSTOM_HEADERS").ok() {
            Some(raw) => parse_custom_headers(&raw)?,
            None => vec![],
        };

        let env_model = std::env::var("CHATTER_MODEL").ok();
        let file_model = UserConfig::load()?.model;
        let model = resolve_model(env_model, file_model, DEFAULT_MODEL);

        Ok(Self {
            base_url,
            custom_headers,
            model,
        })
    }

    pub fn api_config(&self) -> ApiConfig {
        ApiConfig {
            base_url: self.base_url.clone(),
            custom_headers: self.custom_headers.clone(),
            model: self.model.clone(),
        }
    }
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

fn resolve_model(env: Option<String>, file: Option<String>, default: &str) -> String {
    env.or(file).unwrap_or_else(|| default.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn api_config_clones_config_values() {
        let config = Config {
            base_url: "https://example.com/v1".to_string(),
            custom_headers: vec![("X-Key".to_string(), "secret".to_string())],
            model: "model".to_string(),
        };

        let api_config = config.api_config();

        assert_eq!(api_config.base_url, "https://example.com/v1");
        assert_eq!(api_config.custom_headers, vec![("X-Key".to_string(), "secret".to_string())]);
        assert_eq!(api_config.model, "model");
    }

    #[test]
    fn env_model_beats_file_model() {
        let model = resolve_model(
            Some("env-model".to_string()),
            Some("file-model".to_string()),
            "default",
        );
        assert_eq!(model, "env-model");
    }

    #[test]
    fn file_model_used_when_env_missing() {
        let model = resolve_model(None, Some("file-model".to_string()), "default");
        assert_eq!(model, "file-model");
    }

    #[test]
    fn default_used_when_env_and_file_missing() {
        let model = resolve_model(None, None, "default");
        assert_eq!(model, "default");
    }

    #[test]
    fn parse_custom_headers_empty_string() {
        assert_eq!(parse_custom_headers("").unwrap(), vec![]);
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
}
