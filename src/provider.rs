use std::str::FromStr;

use anyhow::{Result, bail};
use serde::{Deserialize, Serialize};

#[derive(Copy, Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Provider {
    Anthropic,
    OpenRouter,
}

impl Provider {
    pub fn default_origin(self) -> &'static str {
        match self {
            Self::Anthropic  => "https://api.anthropic.com",
            Self::OpenRouter => "https://openrouter.ai",
        }
    }

    pub fn default_model(self) -> &'static str {
        match self {
            Self::Anthropic  => "claude-sonnet-4-5",
            Self::OpenRouter => "openrouter/auto",
        }
    }

    pub fn chat_path(self) -> &'static str {
        match self {
            Self::Anthropic  => "/v1/messages",
            Self::OpenRouter => "/api/v1/chat/completions",
        }
    }

    pub fn models_path(self) -> &'static str {
        match self {
            Self::Anthropic  => "/v1/models",
            Self::OpenRouter => "/api/v1/models",
        }
    }
}

impl FromStr for Provider {
    type Err = anyhow::Error;

    fn from_str(s: &str) -> Result<Self> {
        match s.to_lowercase().as_str() {
            "anthropic"   => Ok(Self::Anthropic),
            "openrouter"  => Ok(Self::OpenRouter),
            other => bail!("unknown provider \"{other}\" (expected: anthropic, openrouter)"),
        }
    }
}
