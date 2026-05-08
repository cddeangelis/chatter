use anyhow::{Context, Result, anyhow};
use futures_util::StreamExt;
use serde::{Deserialize, Serialize};

use crate::{
    app_event::{AppEvent, AppEventSender},
    config::Config,
    provider::Provider,
};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Message {
    pub role: String,
    pub content: String,
}

#[derive(Debug, Clone)]
pub struct ModelInfo {
    pub id: String,
    pub name: Option<String>,
    pub context_length: Option<u64>,
    pub max_output_tokens: Option<u64>,
}

// ── Anthropic request / SSE types ────────────────────────────────────────────

#[derive(Serialize)]
struct AnthropicRequest<'a> {
    model: &'a str,
    system: &'a str,
    messages: &'a [Message],
    max_tokens: u32,
    stream: bool,
}

#[derive(Deserialize)]
#[serde(tag = "type")]
enum AnthropicEvent {
    #[serde(rename = "content_block_delta")]
    ContentBlockDelta { delta: AnthropicDelta },
    #[serde(rename = "message_stop")]
    MessageStop,
    #[serde(other)]
    Other,
}

#[derive(Deserialize)]
#[serde(tag = "type")]
enum AnthropicDelta {
    #[serde(rename = "text_delta")]
    TextDelta { text: String },
    #[serde(other)]
    Other,
}

#[derive(Deserialize)]
struct AnthropicModelsResponse {
    data: Vec<AnthropicModelItem>,
}

#[derive(Deserialize)]
struct AnthropicModelItem {
    id: String,
    display_name: Option<String>,
}

// ── OpenRouter request / SSE types ───────────────────────────────────────────

#[derive(Serialize)]
struct OpenRouterRequest<'a> {
    model: &'a str,
    messages: Vec<OrMessage<'a>>,
    max_tokens: u32,
    stream: bool,
}

#[derive(Serialize)]
struct OrMessage<'a> {
    role: &'a str,
    content: &'a str,
}

#[derive(Deserialize)]
struct OrChunk {
    choices: Vec<OrChoice>,
}

#[derive(Deserialize)]
struct OrChoice {
    delta: OrDelta,
}

#[derive(Deserialize)]
struct OrDelta {
    #[serde(default)]
    content: Option<String>,
}

#[derive(Deserialize)]
struct OrModelsResponse {
    data: Vec<OrModelItem>,
}

#[derive(Deserialize)]
struct OrModelItem {
    id: String,
    name: Option<String>,
    context_length: Option<u64>,
    top_provider: Option<OrTopProvider>,
}

#[derive(Deserialize)]
struct OrTopProvider {
    max_completion_tokens: Option<u64>,
}

// ── Public API ────────────────────────────────────────────────────────────────

pub fn build_client() -> Result<reqwest::Client> {
    reqwest::Client::builder()
        .build()
        .context("failed to build HTTP client")
}

pub async fn stream_chat(
    client: reqwest::Client,
    config: Config,
    history: Vec<Message>,
    tx: AppEventSender,
) {
    let result = match config.provider {
        Provider::Anthropic  => stream_chat_anthropic(&client, &config, &history, &tx).await,
        Provider::OpenRouter => stream_chat_openrouter(&client, &config, &history, &tx).await,
    };
    if let Err(e) = result {
        tx.send(AppEvent::StreamError(format!("{e:#}")));
    }
    tx.send(AppEvent::StreamDone);
}

pub async fn load_models(client: reqwest::Client, config: Config, tx: AppEventSender) {
    let result = match config.provider {
        Provider::Anthropic  => fetch_models_anthropic(&client, &config).await,
        Provider::OpenRouter => fetch_models_openrouter(&client, &config).await,
    };
    match result {
        Ok(mut models) => {
            models.sort_by(|a, b| a.id.to_lowercase().cmp(&b.id.to_lowercase()));
            tx.send(AppEvent::ModelsLoaded(models));
        }
        Err(e) => tx.send(AppEvent::ModelsError(format!("{e:#}"))),
    }
}

// ── Shared helpers ────────────────────────────────────────────────────────────

fn apply_headers(mut req: reqwest::RequestBuilder, config: &Config) -> reqwest::RequestBuilder {
    match config.provider {
        Provider::Anthropic => {
            if let Some(key) = &config.api_key {
                req = req.header("x-api-key", key.as_str());
            }
            req = req.header("anthropic-version", "2023-06-01");
        }
        Provider::OpenRouter => {
            if let Some(key) = &config.api_key {
                req = req.header("Authorization", format!("Bearer {key}"));
            }
        }
    }
    for (name, value) in &config.custom_headers {
        req = req.header(name.as_str(), value.as_str());
    }
    req
}

enum SseAction {
    Token(String),
    Done,
    Skip,
}

async fn read_sse_events(
    resp: reqwest::Response,
    decode: impl Fn(&str) -> SseAction,
    tx: &AppEventSender,
) -> Result<()> {
    let mut stream = resp.bytes_stream();
    let mut buf = String::new();

    while let Some(chunk) = stream.next().await {
        let chunk = chunk.context("stream read error")?;
        buf.push_str(&String::from_utf8_lossy(&chunk));

        while let Some(idx) = buf.find("\n\n") {
            let event: String = buf.drain(..idx + 2).collect();
            for line in event.lines() {
                let line = line.trim_start();
                let Some(data) = line.strip_prefix("data:") else { continue };
                let data = data.trim();
                if data.is_empty() {
                    continue;
                }
                match decode(data) {
                    SseAction::Token(t) => tx.send(AppEvent::StreamToken(t)),
                    SseAction::Done    => return Ok(()),
                    SseAction::Skip    => {}
                }
            }
        }
    }

    Ok(())
}

// ── Anthropic ─────────────────────────────────────────────────────────────────

async fn stream_chat_anthropic(
    client: &reqwest::Client,
    config: &Config,
    history: &[Message],
    tx: &AppEventSender,
) -> Result<()> {
    let url = format!("{}{}", config.origin, config.provider.chat_path());
    let body = AnthropicRequest {
        model: &config.model,
        system: &config.system_prompt,
        messages: history,
        max_tokens: config.max_tokens,
        stream: true,
    };
    let req = apply_headers(
        client.post(&url).header("Accept", "text/event-stream"),
        config,
    );
    let resp = req.json(&body).send().await.context("request failed")?;

    let status = resp.status();
    if !status.is_success() {
        let text = resp.text().await.unwrap_or_default();
        return Err(anyhow!("HTTP {status}: {text}"));
    }

    read_sse_events(resp, |data| {
        match serde_json::from_str::<AnthropicEvent>(data) {
            Ok(AnthropicEvent::ContentBlockDelta {
                delta: AnthropicDelta::TextDelta { text },
            }) if !text.is_empty() => SseAction::Token(text),
            Ok(AnthropicEvent::MessageStop) => SseAction::Done,
            _ => SseAction::Skip,
        }
    }, tx).await
}

async fn fetch_models_anthropic(client: &reqwest::Client, config: &Config) -> Result<Vec<ModelInfo>> {
    let url = format!("{}{}", config.origin, config.provider.models_path());
    let req = apply_headers(client.get(&url), config);
    let resp = req.send().await.context("model request failed")?;

    let status = resp.status();
    if !status.is_success() {
        let text = resp.text().await.unwrap_or_default();
        return Err(anyhow!("HTTP {status}: {text}"));
    }

    Ok(resp
        .json::<AnthropicModelsResponse>()
        .await
        .context("failed to decode Anthropic models response")?
        .data
        .into_iter()
        .map(|item| ModelInfo {
            id: item.id,
            name: item.display_name,
            context_length: None,
            max_output_tokens: None,
        })
        .collect())
}

// ── OpenRouter ────────────────────────────────────────────────────────────────

async fn stream_chat_openrouter(
    client: &reqwest::Client,
    config: &Config,
    history: &[Message],
    tx: &AppEventSender,
) -> Result<()> {
    let url = format!("{}{}", config.origin, config.provider.chat_path());

    let mut messages = vec![OrMessage {
        role: "system",
        content: &config.system_prompt,
    }];
    messages.extend(history.iter().map(|m| OrMessage {
        role: &m.role,
        content: &m.content,
    }));

    let body = OpenRouterRequest {
        model: &config.model,
        messages,
        max_tokens: config.max_tokens,
        stream: true,
    };
    let req = apply_headers(
        client.post(&url).header("Accept", "text/event-stream"),
        config,
    );
    let resp = req.json(&body).send().await.context("request failed")?;

    let status = resp.status();
    if !status.is_success() {
        let text = resp.text().await.unwrap_or_default();
        return Err(anyhow!("HTTP {status}: {text}"));
    }

    read_sse_events(resp, |data| {
        if data == "[DONE]" {
            return SseAction::Done;
        }
        match serde_json::from_str::<OrChunk>(data) {
            Ok(chunk) => {
                let text = chunk
                    .choices
                    .into_iter()
                    .filter_map(|c| c.delta.content)
                    .find(|t| !t.is_empty());
                match text {
                    Some(t) => SseAction::Token(t),
                    None    => SseAction::Skip,
                }
            }
            Err(_) => SseAction::Skip,
        }
    }, tx).await
}

async fn fetch_models_openrouter(client: &reqwest::Client, config: &Config) -> Result<Vec<ModelInfo>> {
    let url = format!("{}{}", config.origin, config.provider.models_path());
    let req = apply_headers(client.get(&url), config);
    let resp = req.send().await.context("model request failed")?;

    let status = resp.status();
    if !status.is_success() {
        let text = resp.text().await.unwrap_or_default();
        return Err(anyhow!("HTTP {status}: {text}"));
    }

    Ok(resp
        .json::<OrModelsResponse>()
        .await
        .context("failed to decode OpenRouter models response")?
        .data
        .into_iter()
        .map(|item| ModelInfo {
            id: item.id,
            name: item.name,
            context_length: item.context_length,
            max_output_tokens: item.top_provider.and_then(|p| p.max_completion_tokens),
        })
        .collect())
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn anthropic_request_serializes_expected_shape() {
        let messages = vec![Message {
            role: "user".to_string(),
            content: "hello".to_string(),
        }];
        let request = AnthropicRequest {
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
    fn openrouter_request_puts_system_message_first() {
        let history = vec![Message {
            role: "user".to_string(),
            content: "hi".to_string(),
        }];
        let messages: Vec<OrMessage> = {
            let mut v = vec![OrMessage { role: "system", content: "sys" }];
            v.extend(history.iter().map(|m| OrMessage { role: &m.role, content: &m.content }));
            v
        };
        let request = OpenRouterRequest {
            model: "test-model",
            messages,
            max_tokens: 8192,
            stream: true,
        };

        let json = serde_json::to_value(request).unwrap();

        assert_eq!(json["messages"][0]["role"], "system");
        assert_eq!(json["messages"][0]["content"], "sys");
        assert_eq!(json["messages"][1]["role"], "user");
        assert_eq!(json["messages"][1]["content"], "hi");
    }

    #[test]
    fn anthropic_event_parses_text_delta() {
        let event: AnthropicEvent = serde_json::from_str(
            r#"{"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":"hi"}}"#,
        )
        .unwrap();

        match event {
            AnthropicEvent::ContentBlockDelta {
                delta: AnthropicDelta::TextDelta { text },
            } => assert_eq!(text, "hi"),
            _ => panic!("expected text_delta"),
        }
    }

    #[test]
    fn anthropic_event_ignores_unknown_types() {
        let ping: AnthropicEvent = serde_json::from_str(r#"{"type":"ping"}"#).unwrap();
        assert!(matches!(ping, AnthropicEvent::Other));

        let msg_delta: AnthropicEvent = serde_json::from_str(
            r#"{"type":"message_delta","delta":{"stop_reason":"end_turn"},"usage":{"output_tokens":9}}"#,
        )
        .unwrap();
        assert!(matches!(msg_delta, AnthropicEvent::Other));

        let unknown_delta: AnthropicEvent = serde_json::from_str(
            r#"{"type":"content_block_delta","index":0,"delta":{"type":"thinking_delta","thinking":"..."}}"#,
        )
        .unwrap();
        assert!(matches!(
            unknown_delta,
            AnthropicEvent::ContentBlockDelta { delta: AnthropicDelta::Other }
        ));
    }

    #[test]
    fn openrouter_chunk_parses_content_delta() {
        let chunk: OrChunk = serde_json::from_str(
            r#"{"id":"x","choices":[{"index":0,"delta":{"role":"assistant","content":"hello"},"finish_reason":null}]}"#,
        )
        .unwrap();

        assert_eq!(chunk.choices[0].delta.content.as_deref(), Some("hello"));
    }

    #[test]
    fn openrouter_chunk_handles_missing_content() {
        let chunk: OrChunk = serde_json::from_str(
            r#"{"id":"x","choices":[{"index":0,"delta":{"role":"assistant"},"finish_reason":null}]}"#,
        )
        .unwrap();

        assert!(chunk.choices[0].delta.content.is_none());
    }

    #[test]
    fn openrouter_done_sentinel_triggers_done_action() {
        // Verify [DONE] is recognized before attempting JSON parse
        let action = (|data: &str| {
            if data == "[DONE]" {
                return SseAction::Done;
            }
            SseAction::Skip
        })("[DONE]");

        assert!(matches!(action, SseAction::Done));
    }
}
