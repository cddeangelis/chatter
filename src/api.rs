use anyhow::{Context, Result, anyhow};
use futures_util::StreamExt;
use serde::{Deserialize, Serialize};

use crate::app_event::{AppEvent, AppEventSender};

pub const DEFAULT_MODEL: &str = "Claude-Sonnet-4.6";

pub const SYSTEM_PROMPT: &str = "You and the user are having a conversation. This is not a user-assistant interaction, simply a conversation between two minds.";

const MAX_TOKENS: u32 = 8192;
const ANTHROPIC_VERSION: &str = "2023-06-01";

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Message {
    pub role: String,
    pub content: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct ModelInfo {
    pub id: String,
    pub owned_by: Option<String>,
    pub context_length: Option<u64>,
    pub max_output_tokens: Option<u64>,
    pub capacity: Option<String>,
    pub capabilities: Option<ModelCapabilities>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct ModelCapabilities {
    pub chat: bool,
}

#[derive(Clone)]
pub struct ApiConfig {
    pub base_url: String,
    pub custom_headers: Vec<(String, String)>,
    pub model: String,
}

#[derive(Serialize)]
struct MessagesRequest<'a> {
    model: &'a str,
    system: &'a str,
    messages: &'a [Message],
    max_tokens: u32,
    stream: bool,
}

#[derive(Deserialize)]
#[serde(tag = "type")]
enum StreamEvent {
    #[serde(rename = "content_block_delta")]
    ContentBlockDelta { delta: ContentDelta },
    #[serde(rename = "message_stop")]
    MessageStop,
    #[serde(other)]
    Other,
}

#[derive(Deserialize)]
#[serde(tag = "type")]
enum ContentDelta {
    #[serde(rename = "text_delta")]
    TextDelta { text: String },
    #[serde(other)]
    Other,
}

#[derive(Deserialize)]
struct ModelsResponse {
    data: Vec<ModelInfo>,
}

pub fn build_client() -> Result<reqwest::Client> {
    reqwest::Client::builder()
        .build()
        .context("failed to build HTTP client")
}

pub async fn stream_chat(
    client: reqwest::Client,
    config: ApiConfig,
    history: Vec<Message>,
    tx: AppEventSender,
) {
    if let Err(e) = stream_chat_inner(client, config, history, &tx).await {
        tx.send(AppEvent::StreamError(format!("{e:#}")));
    }
    tx.send(AppEvent::StreamDone);
}

pub async fn load_models(client: reqwest::Client, config: ApiConfig, tx: AppEventSender) {
    match list_models(client, &config).await {
        Ok(models) => tx.send(AppEvent::ModelsLoaded(models)),
        Err(e) => tx.send(AppEvent::ModelsError(format!("{e:#}"))),
    }
}

async fn stream_chat_inner(
    client: reqwest::Client,
    config: ApiConfig,
    history: Vec<Message>,
    tx: &AppEventSender,
) -> Result<()> {
    let body = MessagesRequest {
        model: &config.model,
        system: SYSTEM_PROMPT,
        messages: &history,
        max_tokens: MAX_TOKENS,
        stream: true,
    };

    let url = format!("{}/messages", config.base_url);
    let mut req = client
        .post(url)
        .header("anthropic-version", ANTHROPIC_VERSION)
        .header("Accept", "text/event-stream");
    for (name, value) in &config.custom_headers {
        req = req.header(name.as_str(), value.as_str());
    }
    let resp = req
        .json(&body)
        .send()
        .await
        .context("request failed")?;

    let status = resp.status();
    if !status.is_success() {
        let text = resp.text().await.unwrap_or_default();
        return Err(anyhow!("HTTP {status}: {text}"));
    }

    let mut stream = resp.bytes_stream();
    let mut buf = String::new();

    while let Some(chunk) = stream.next().await {
        let chunk = chunk.context("stream read error")?;
        buf.push_str(&String::from_utf8_lossy(&chunk));

        while let Some(idx) = buf.find("\n\n") {
            let event: String = buf.drain(..idx + 2).collect();
            for line in event.lines() {
                let line = line.trim_start();
                let Some(data) = line.strip_prefix("data:") else {
                    continue;
                };
                let data = data.trim();
                if data.is_empty() {
                    continue;
                }
                match serde_json::from_str::<StreamEvent>(data) {
                    Ok(StreamEvent::ContentBlockDelta {
                        delta: ContentDelta::TextDelta { text },
                    }) => {
                        if !text.is_empty() {
                            tx.send(AppEvent::StreamToken(text));
                        }
                    }
                    Ok(StreamEvent::MessageStop) => return Ok(()),
                    Ok(_) => {}
                    Err(_) => {}
                }
            }
        }
    }

    Ok(())
}

async fn list_models(client: reqwest::Client, config: &ApiConfig) -> Result<Vec<ModelInfo>> {
    let url = format!("{}/models", config.base_url);
    let mut req = client.get(url);
    for (name, value) in &config.custom_headers {
        req = req.header(name.as_str(), value.as_str());
    }
    let resp = req.send().await.context("model request failed")?;

    let status = resp.status();
    if !status.is_success() {
        let text = resp.text().await.unwrap_or_default();
        return Err(anyhow!("HTTP {status}: {text}"));
    }

    let models = resp
        .json::<ModelsResponse>()
        .await
        .context("failed to decode models response")?
        .data;

    Ok(chat_models(models))
}

fn chat_models(models: Vec<ModelInfo>) -> Vec<ModelInfo> {
    let mut models = models
        .into_iter()
        .filter(|model| {
            model
                .capabilities
                .as_ref()
                .is_some_and(|capabilities| capabilities.chat)
        })
        .collect::<Vec<_>>();

    models.sort_by(|a, b| a.id.to_lowercase().cmp(&b.id.to_lowercase()));
    models
}

#[cfg(test)]
mod tests {
    use super::*;

    fn model(id: &str, chat: Option<bool>) -> ModelInfo {
        ModelInfo {
            id: id.to_string(),
            owned_by: None,
            context_length: None,
            max_output_tokens: None,
            capacity: None,
            capabilities: chat.map(|chat| ModelCapabilities { chat }),
        }
    }

    #[test]
    fn chat_models_keeps_chat_capable_models_sorted_case_insensitively() {
        let models = chat_models(vec![
            model("zeta", Some(true)),
            model("VisionOnly", Some(false)),
            model("alpha", Some(true)),
            model("NoCapabilities", None),
            model("Beta", Some(true)),
        ]);

        let ids = models.into_iter().map(|model| model.id).collect::<Vec<_>>();
        assert_eq!(ids, vec!["alpha", "Beta", "zeta"]);
    }

    #[test]
    fn messages_request_serializes_expected_shape() {
        let messages = vec![Message {
            role: "user".to_string(),
            content: "hello".to_string(),
        }];
        let request = MessagesRequest {
            model: "test-model",
            system: "sys",
            messages: &messages,
            max_tokens: 1234,
            stream: true,
        };

        let json = serde_json::to_value(request).unwrap();

        assert_eq!(json["model"], "test-model");
        assert_eq!(json["system"], "sys");
        assert_eq!(json["max_tokens"], 1234);
        assert_eq!(json["stream"], true);
        assert_eq!(json["messages"][0]["role"], "user");
        assert_eq!(json["messages"][0]["content"], "hello");
    }

    #[test]
    fn stream_event_parses_text_delta() {
        let event: StreamEvent = serde_json::from_str(
            r#"{"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":"hi"}}"#,
        )
        .unwrap();

        match event {
            StreamEvent::ContentBlockDelta {
                delta: ContentDelta::TextDelta { text },
            } => assert_eq!(text, "hi"),
            _ => panic!("expected text_delta"),
        }
    }

    #[test]
    fn stream_event_ignores_unknown_types() {
        let ping: StreamEvent = serde_json::from_str(r#"{"type":"ping"}"#).unwrap();
        assert!(matches!(ping, StreamEvent::Other));

        let msg_delta: StreamEvent = serde_json::from_str(
            r#"{"type":"message_delta","delta":{"stop_reason":"end_turn"},"usage":{"output_tokens":9}}"#,
        )
        .unwrap();
        assert!(matches!(msg_delta, StreamEvent::Other));

        let unknown_delta: StreamEvent = serde_json::from_str(
            r#"{"type":"content_block_delta","index":0,"delta":{"type":"thinking_delta","thinking":"..."}}"#,
        )
        .unwrap();
        assert!(matches!(
            unknown_delta,
            StreamEvent::ContentBlockDelta {
                delta: ContentDelta::Other
            }
        ));
    }
}
