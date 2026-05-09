use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::text::Line;

use crate::{
    api::{Message, ModelInfo},
    app_event::{AppEvent, AppEventSender},
    commands::{CommandError, SlashCommand, parse_slash_command},
    provider::Provider,
    ui,
};

pub enum ViewMode {
    Chat,
    ModelPicker,
    AuthWizard,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum AuthProviderChoice {
    Anthropic,
    OpenRouter,
    Custom,
}

impl AuthProviderChoice {
    pub fn label(self) -> &'static str {
        match self {
            Self::Anthropic  => "Anthropic",
            Self::OpenRouter => "OpenRouter",
            Self::Custom     => "Custom (OpenAI-compatible)",
        }
    }

    pub fn description(self) -> &'static str {
        match self {
            Self::Anthropic  => "Claude models via the Anthropic API",
            Self::OpenRouter => "200+ models via OpenRouter",
            Self::Custom     => "Any OpenAI-compatible endpoint (Together, Groq, llama.cpp, …)",
        }
    }
}

pub const AUTH_PROVIDER_CHOICES: &[AuthProviderChoice] = &[
    AuthProviderChoice::Anthropic,
    AuthProviderChoice::OpenRouter,
    AuthProviderChoice::Custom,
];

#[derive(Clone, PartialEq, Eq)]
pub enum AuthStep {
    SelectProvider,
    EnterOrigin,
    EnterApiKey,
}

pub struct AuthWizard {
    pub step: AuthStep,
    pub provider_idx: usize,
    pub origin: String,
    pub origin_cursor: usize,
    pub api_key: String,
    pub api_key_cursor: usize,
    pub error: Option<String>,
}

impl Default for AuthWizard {
    fn default() -> Self {
        Self {
            step: AuthStep::SelectProvider,
            provider_idx: 0,
            origin: String::new(),
            origin_cursor: 0,
            api_key: String::new(),
            api_key_cursor: 0,
            error: None,
        }
    }
}

pub struct ModelPicker {
    pub selected: usize,
    pub scroll: usize,
    pub loading: bool,
    pub error: Option<String>,
}

pub enum StatusKind {
    Info,
    Error,
}

pub struct Status {
    pub text: String,
    pub kind: StatusKind,
}

#[derive(Default)]
pub struct LiveCell {
    pub raw: String,
    pub committed_bytes: usize,
    pub prefix_emitted: bool,
}

pub struct App {
    pub messages: Vec<Message>,
    pub input: String,
    pub cursor: usize,
    pub streaming: bool,
    pub status: Option<Status>,
    pub spinner_idx: usize,
    pub current_model: String,
    pub models: Vec<ModelInfo>,
    pub mode: ViewMode,
    pub model_picker: ModelPicker,
    pub auth_wizard: AuthWizard,
    pub live: Option<LiveCell>,
    pub dirty: bool,
}

impl App {
    pub fn with_messages(
        current_model: String,
        messages: Vec<Message>,
        session_id: Option<&str>,
    ) -> Self {
        let status = match session_id {
            Some(id) => format!("session: {id} · model: {current_model}"),
            None => format!("model: {current_model}"),
        };

        Self {
            messages,
            input: String::new(),
            cursor: 0,
            streaming: false,
            status: Some(Status::info(status)),
            spinner_idx: 0,
            current_model,
            models: Vec::new(),
            mode: ViewMode::Chat,
            model_picker: ModelPicker {
                selected: 0,
                scroll: 0,
                loading: false,
                error: None,
            },
            auth_wizard: AuthWizard::default(),
            live: None,
            dirty: true,
        }
    }

    pub fn tick(&mut self) {
        if self.streaming || self.model_picker.loading {
            self.spinner_idx = (self.spinner_idx + 1) % crate::ui::SPINNER_FRAMES.len();
            self.dirty = true;
        }
    }

    pub fn mark_dirty(&mut self) {
        self.dirty = true;
    }

    pub fn take_dirty(&mut self) -> bool {
        std::mem::replace(&mut self.dirty, false)
    }

    pub fn handle_key(&mut self, key: KeyEvent, tx: &AppEventSender) {
        if key.modifiers.contains(KeyModifiers::CONTROL) && key.code == KeyCode::Char('c') {
            tx.send(AppEvent::Quit);
            return;
        }

        match self.mode {
            ViewMode::Chat => self.handle_chat_key(key, tx),
            ViewMode::ModelPicker => self.handle_model_picker_key(key, tx),
            ViewMode::AuthWizard => self.handle_auth_wizard_key(key, tx),
        }
    }

    fn handle_chat_key(&mut self, key: KeyEvent, tx: &AppEventSender) {
        match key.code {
            KeyCode::Esc => tx.send(AppEvent::Quit),
            KeyCode::Enter if !self.streaming => {
                let text = self.input.trim().to_string();
                if text.is_empty() {
                    return;
                }

                if let Some(command_text) = text.strip_prefix('/') {
                    self.clear_input();
                    self.handle_slash_command(command_text, tx);
                    return;
                }

                self.clear_input();
                tx.send(AppEvent::Submit(text));
            }
            KeyCode::Backspace if !self.streaming => {
                self.backspace();
            }
            KeyCode::Left if !self.streaming => {
                self.move_left();
            }
            KeyCode::Right if !self.streaming => {
                self.move_right();
            }
            KeyCode::Home if !self.streaming => {
                self.move_home();
            }
            KeyCode::End if !self.streaming => {
                self.move_end();
            }
            KeyCode::Char(c) if !self.streaming => {
                self.insert_char(c);
            }
            _ => {}
        }
    }

    pub fn clear_input(&mut self) {
        self.input.clear();
        self.cursor = 0;
    }

    fn insert_char(&mut self, c: char) {
        self.input.insert(self.cursor, c);
        self.cursor += c.len_utf8();
    }

    fn backspace(&mut self) {
        if self.cursor == 0 {
            return;
        }
        let prev = self.input[..self.cursor]
            .char_indices()
            .next_back()
            .map(|(i, _)| i)
            .unwrap_or(0);
        self.input.remove(prev);
        self.cursor = prev;
    }

    fn move_left(&mut self) {
        if self.cursor == 0 {
            return;
        }
        self.cursor = self.input[..self.cursor]
            .char_indices()
            .next_back()
            .map(|(i, _)| i)
            .unwrap_or(0);
    }

    fn move_right(&mut self) {
        if self.cursor >= self.input.len() {
            return;
        }
        let advance = self.input[self.cursor..]
            .chars()
            .next()
            .map(|c| c.len_utf8())
            .unwrap_or(0);
        self.cursor += advance;
    }

    fn move_home(&mut self) {
        self.cursor = 0;
    }

    fn move_end(&mut self) {
        self.cursor = self.input.len();
    }

    fn handle_slash_command(&mut self, command_text: &str, tx: &AppEventSender) {
        match parse_slash_command(command_text) {
            Ok(SlashCommand::Auth) => self.open_auth_wizard(),
            Ok(SlashCommand::Clear) => tx.send(AppEvent::Clear),
            Ok(SlashCommand::Exit) => tx.send(AppEvent::Quit),
            Ok(SlashCommand::Model) => {
                self.open_model_picker();
                if self.models.is_empty() {
                    self.model_picker.loading = true;
                    tx.send(AppEvent::LoadModels);
                }
            }
            Err(CommandError::Unknown(name)) => {
                self.status = Some(Status::info(format!("unknown command: /{name}")));
            }
            Err(CommandError::Empty) => {
                self.status = Some(Status::info("type /auth, /clear, /exit, or /model"));
            }
        }
    }

    fn handle_model_picker_key(&mut self, key: KeyEvent, tx: &AppEventSender) {
        match key.code {
            KeyCode::Esc => {
                self.close_model_picker();
            }
            KeyCode::Up | KeyCode::Char('k') => {
                self.move_model_selection(-1);
            }
            KeyCode::Down | KeyCode::Char('j') => {
                self.move_model_selection(1);
            }
            KeyCode::PageUp => {
                self.move_model_selection(-10);
            }
            KeyCode::PageDown => {
                self.move_model_selection(10);
            }
            KeyCode::Home => {
                self.model_picker.selected = 0;
                self.keep_selected_model_visible();
            }
            KeyCode::End => {
                self.model_picker.selected = self.models.len().saturating_sub(1);
                self.keep_selected_model_visible();
            }
            KeyCode::Enter if !self.models.is_empty() => {
                let model = self.models[self.model_picker.selected].id.clone();
                self.set_current_model(model.clone());
                self.close_model_picker();
                tx.send(AppEvent::SelectModel(model));
            }
            _ => {}
        }
    }

    pub fn push_user(&mut self, content: String) {
        self.messages.push(Message {
            role: "user".into(),
            content,
        });
    }

    pub fn begin_assistant(&mut self) {
        self.streaming = true;
        self.status = Some(Status::info("thinking"));
        self.live = Some(LiveCell::default());
    }

    /// Append streamed token text to the live cell. Returns any complete lines
    /// (terminated by `\n`) that should be flushed to scrollback.
    pub fn append_assistant(&mut self, token: &str) -> Vec<Line<'static>> {
        let Some(live) = self.live.as_mut() else {
            return Vec::new();
        };
        live.raw.push_str(token);

        if self
            .status
            .as_ref()
            .is_some_and(|status| status.text == "thinking")
        {
            self.status = None;
        }

        let tail = &live.raw[live.committed_bytes..];
        let Some(last_nl) = tail.rfind('\n') else {
            return Vec::new();
        };
        let finalize_end = live.committed_bytes + last_nl + 1;
        let chunk = live.raw[live.committed_bytes..finalize_end].to_string();
        let lines = ui::render_assistant_chunk(&chunk, !live.prefix_emitted);
        live.prefix_emitted = true;
        live.committed_bytes = finalize_end;
        lines
    }

    /// Finalize streaming. Returns the trailing partial line (if any) for
    /// flushing to scrollback. Persists the full assistant message.
    pub fn finish_assistant(&mut self) -> Vec<Line<'static>> {
        self.streaming = false;
        if !self.status_is_error() {
            self.status = None;
        }

        let Some(live) = self.live.take() else {
            return Vec::new();
        };

        let tail = live.raw[live.committed_bytes..].to_string();
        let trailing = if tail.is_empty() {
            Vec::new()
        } else {
            ui::render_assistant_chunk(&tail, !live.prefix_emitted)
        };

        if !live.raw.is_empty() {
            self.messages.push(Message {
                role: "assistant".into(),
                content: live.raw,
            });
        }
        trailing
    }

    pub fn set_error(&mut self, msg: String) {
        self.streaming = false;
        self.live = None;
        self.status = Some(Status::error(format!("error: {msg}")));
    }

    pub fn set_models(&mut self, models: Vec<ModelInfo>) {
        self.models = models;
        self.model_picker.loading = false;
        self.model_picker.error = None;
        self.select_current_model();
        self.keep_selected_model_visible();
        self.status = Some(Status::info(format!(
            "loaded {} chat models",
            self.models.len()
        )));
    }

    pub fn set_model_load_error(&mut self, msg: String) {
        self.model_picker.loading = false;
        self.model_picker.error = Some(msg.clone());
        self.status = Some(Status::error(format!("model load failed: {msg}")));
    }

    pub fn clear_for_new_session(&mut self, session_id: &str) {
        self.messages.clear();
        self.clear_input();
        self.streaming = false;
        self.live = None;
        self.mode = ViewMode::Chat;
        self.model_picker.loading = false;
        self.model_picker.error = None;
        self.status = Some(Status::info(format!(
            "session: {session_id} · model: {}",
            self.current_model
        )));
    }

    fn open_model_picker(&mut self) {
        self.mode = ViewMode::ModelPicker;
        self.model_picker.error = None;
        self.select_current_model();
        self.keep_selected_model_visible();
    }

    fn close_model_picker(&mut self) {
        self.mode = ViewMode::Chat;
        self.model_picker.loading = false;
        self.model_picker.error = None;
    }

    pub fn open_auth_wizard(&mut self) {
        self.auth_wizard = AuthWizard::default();
        self.mode = ViewMode::AuthWizard;
    }

    fn close_auth_wizard(&mut self) {
        self.mode = ViewMode::Chat;
    }

    fn handle_auth_wizard_key(&mut self, key: KeyEvent, tx: &AppEventSender) {
        match self.auth_wizard.step {
            AuthStep::SelectProvider => self.handle_auth_select_provider(key),
            AuthStep::EnterOrigin    => self.handle_auth_enter_origin(key),
            AuthStep::EnterApiKey    => self.handle_auth_enter_api_key(key, tx),
        }
    }

    fn handle_auth_select_provider(&mut self, key: KeyEvent) {
        let max = AUTH_PROVIDER_CHOICES.len() - 1;
        match key.code {
            KeyCode::Esc => self.close_auth_wizard(),
            KeyCode::Up | KeyCode::Char('k') => {
                self.auth_wizard.provider_idx =
                    self.auth_wizard.provider_idx.saturating_sub(1);
            }
            KeyCode::Down | KeyCode::Char('j') => {
                self.auth_wizard.provider_idx =
                    (self.auth_wizard.provider_idx + 1).min(max);
            }
            KeyCode::Home => self.auth_wizard.provider_idx = 0,
            KeyCode::End  => self.auth_wizard.provider_idx = max,
            KeyCode::Enter => {
                self.auth_wizard.error = None;
                let choice = AUTH_PROVIDER_CHOICES[self.auth_wizard.provider_idx];
                self.auth_wizard.step = match choice {
                    AuthProviderChoice::Custom => AuthStep::EnterOrigin,
                    _                          => AuthStep::EnterApiKey,
                };
            }
            _ => {}
        }
    }

    fn handle_auth_enter_origin(&mut self, key: KeyEvent) {
        match key.code {
            KeyCode::Esc => self.close_auth_wizard(),
            KeyCode::Enter => {
                match crate::config::validate_origin(&self.auth_wizard.origin) {
                    Ok(normalized) => {
                        self.auth_wizard.origin = normalized;
                        self.auth_wizard.origin_cursor = self.auth_wizard.origin.len();
                        self.auth_wizard.error = None;
                        self.auth_wizard.step = AuthStep::EnterApiKey;
                    }
                    Err(e) => {
                        self.auth_wizard.error = Some(e.to_string());
                    }
                }
            }
            KeyCode::Backspace => {
                let cursor = self.auth_wizard.origin_cursor;
                if cursor == 0 { return; }
                let prev = self.auth_wizard.origin[..cursor]
                    .char_indices().next_back().map(|(i, _)| i).unwrap_or(0);
                self.auth_wizard.origin.remove(prev);
                self.auth_wizard.origin_cursor = prev;
            }
            KeyCode::Left => {
                let cursor = self.auth_wizard.origin_cursor;
                if cursor > 0 {
                    self.auth_wizard.origin_cursor = self.auth_wizard.origin[..cursor]
                        .char_indices().next_back().map(|(i, _)| i).unwrap_or(0);
                }
            }
            KeyCode::Right => {
                let cursor = self.auth_wizard.origin_cursor;
                if cursor < self.auth_wizard.origin.len() {
                    self.auth_wizard.origin_cursor += self.auth_wizard.origin[cursor..]
                        .chars().next().map(|c| c.len_utf8()).unwrap_or(0);
                }
            }
            KeyCode::Home => self.auth_wizard.origin_cursor = 0,
            KeyCode::End  => self.auth_wizard.origin_cursor = self.auth_wizard.origin.len(),
            KeyCode::Char(c) => {
                self.auth_wizard.error = None;
                self.auth_wizard.origin.insert(self.auth_wizard.origin_cursor, c);
                self.auth_wizard.origin_cursor += c.len_utf8();
            }
            _ => {}
        }
    }

    fn handle_auth_enter_api_key(&mut self, key: KeyEvent, tx: &AppEventSender) {
        match key.code {
            KeyCode::Esc => self.close_auth_wizard(),
            KeyCode::Enter => {
                if self.auth_wizard.api_key.is_empty() {
                    return;
                }
                let choice = AUTH_PROVIDER_CHOICES[self.auth_wizard.provider_idx];
                let provider = match choice {
                    AuthProviderChoice::Anthropic              => Provider::Anthropic,
                    AuthProviderChoice::OpenRouter |
                    AuthProviderChoice::Custom     => Provider::OpenRouter,
                };
                let origin = if choice == AuthProviderChoice::Custom {
                    Some(self.auth_wizard.origin.clone())
                } else {
                    None
                };
                tx.send(AppEvent::AuthSubmit {
                    provider,
                    origin,
                    api_key: self.auth_wizard.api_key.clone(),
                });
                self.close_auth_wizard();
            }
            KeyCode::Backspace => {
                let cursor = self.auth_wizard.api_key_cursor;
                if cursor == 0 { return; }
                let prev = self.auth_wizard.api_key[..cursor]
                    .char_indices().next_back().map(|(i, _)| i).unwrap_or(0);
                self.auth_wizard.api_key.remove(prev);
                self.auth_wizard.api_key_cursor = prev;
            }
            KeyCode::Left => {
                let cursor = self.auth_wizard.api_key_cursor;
                if cursor > 0 {
                    self.auth_wizard.api_key_cursor = self.auth_wizard.api_key[..cursor]
                        .char_indices().next_back().map(|(i, _)| i).unwrap_or(0);
                }
            }
            KeyCode::Right => {
                let cursor = self.auth_wizard.api_key_cursor;
                if cursor < self.auth_wizard.api_key.len() {
                    self.auth_wizard.api_key_cursor += self.auth_wizard.api_key[cursor..]
                        .chars().next().map(|c| c.len_utf8()).unwrap_or(0);
                }
            }
            KeyCode::Home => self.auth_wizard.api_key_cursor = 0,
            KeyCode::End  => self.auth_wizard.api_key_cursor = self.auth_wizard.api_key.len(),
            KeyCode::Char(c) => {
                self.auth_wizard.api_key.insert(self.auth_wizard.api_key_cursor, c);
                self.auth_wizard.api_key_cursor += c.len_utf8();
            }
            _ => {}
        }
    }

    pub fn set_status_info(&mut self, msg: String) {
        self.status = Some(Status::info(msg));
    }

    fn set_current_model(&mut self, model: String) {
        self.current_model = model.clone();
        self.status = Some(Status::info(format!("model: {model}")));
    }

    fn select_current_model(&mut self) {
        if let Some(idx) = self
            .models
            .iter()
            .position(|model| model.id == self.current_model)
        {
            self.model_picker.selected = idx;
        } else {
            self.model_picker.selected = 0;
        }
    }

    fn move_model_selection(&mut self, delta: isize) {
        if self.models.is_empty() {
            return;
        }

        let max = self.models.len() - 1;
        let selected = self.model_picker.selected.saturating_add_signed(delta);
        self.model_picker.selected = selected.min(max);
        self.keep_selected_model_visible();
    }

    fn keep_selected_model_visible(&mut self) {
        let selected = self.model_picker.selected;
        if selected < self.model_picker.scroll {
            self.model_picker.scroll = selected;
        }
    }

    fn status_is_error(&self) -> bool {
        self.status
            .as_ref()
            .is_some_and(|status| matches!(status.kind, StatusKind::Error))
    }
}

impl Status {
    fn info(text: impl Into<String>) -> Self {
        Self {
            text: text.into(),
            kind: StatusKind::Info,
        }
    }

    fn error(text: impl Into<String>) -> Self {
        Self {
            text: text.into(),
            kind: StatusKind::Error,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::sync::mpsc::UnboundedReceiver;

    fn app() -> App {
        App::with_messages("test-model".to_string(), Vec::new(), Some("session-id"))
    }

    fn key(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::empty())
    }

    fn ctrl_key(c: char) -> KeyEvent {
        KeyEvent::new(KeyCode::Char(c), KeyModifiers::CONTROL)
    }

    fn model(id: &str) -> ModelInfo {
        ModelInfo {
            id: id.to_string(),
            name: None,
            context_length: None,
            max_output_tokens: None,
        }
    }

    fn channel() -> (AppEventSender, UnboundedReceiver<AppEvent>) {
        crate::app_event::channel()
    }

    fn drain(rx: &mut UnboundedReceiver<AppEvent>) -> Vec<AppEvent> {
        let mut out = Vec::new();
        while let Ok(ev) = rx.try_recv() {
            out.push(ev);
        }
        out
    }

    #[test]
    fn handle_key_submits_trimmed_input() {
        let mut app = app();
        let (tx, mut rx) = channel();
        app.input = "  hello  ".to_string();

        app.handle_key(key(KeyCode::Enter), &tx);

        let events = drain(&mut rx);
        assert_eq!(events.len(), 1);
        assert!(matches!(&events[0], AppEvent::Submit(t) if t == "hello"));
        assert!(app.input.is_empty());
    }

    #[test]
    fn handle_key_ignores_empty_submit() {
        let mut app = app();
        let (tx, mut rx) = channel();
        app.input = "   ".to_string();

        app.handle_key(key(KeyCode::Enter), &tx);

        assert!(drain(&mut rx).is_empty());
        assert_eq!(app.input, "   ");
    }

    #[test]
    fn handle_key_routes_exit_command_to_quit() {
        let mut app = app();
        let (tx, mut rx) = channel();
        app.input = "/quit".to_string();

        app.handle_key(key(KeyCode::Enter), &tx);

        let events = drain(&mut rx);
        assert!(matches!(events.as_slice(), [AppEvent::Quit]));
        assert!(app.input.is_empty());
    }

    #[test]
    fn handle_key_routes_clear_command_to_clear_event() {
        let mut app = app();
        let (tx, mut rx) = channel();
        app.input = "/clear".to_string();

        app.handle_key(key(KeyCode::Enter), &tx);

        let events = drain(&mut rx);
        assert!(matches!(events.as_slice(), [AppEvent::Clear]));
    }

    #[test]
    fn handle_key_opens_model_picker_and_loads_when_empty() {
        let mut app = app();
        let (tx, mut rx) = channel();
        app.input = "/model".to_string();

        app.handle_key(key(KeyCode::Enter), &tx);

        let events = drain(&mut rx);
        assert!(matches!(events.as_slice(), [AppEvent::LoadModels]));
        assert!(matches!(app.mode, ViewMode::ModelPicker));
        assert!(app.model_picker.loading);
    }

    #[test]
    fn handle_key_opens_model_picker_without_loading_cached_models() {
        let mut app = app();
        let (tx, mut rx) = channel();
        app.models = vec![model("test-model")];
        app.input = "/model".to_string();

        app.handle_key(key(KeyCode::Enter), &tx);

        assert!(drain(&mut rx).is_empty());
        assert!(matches!(app.mode, ViewMode::ModelPicker));
        assert!(!app.model_picker.loading);
    }

    #[test]
    fn streaming_blocks_text_editing_and_submit() {
        let mut app = app();
        let (tx, mut rx) = channel();
        app.input = "hello".to_string();
        app.cursor = app.input.len();
        app.streaming = true;

        app.handle_key(key(KeyCode::Char('!')), &tx);
        app.handle_key(key(KeyCode::Enter), &tx);
        app.handle_key(key(KeyCode::Left), &tx);
        app.handle_key(key(KeyCode::Home), &tx);

        assert!(drain(&mut rx).is_empty());
        assert_eq!(app.input, "hello");
        assert_eq!(app.cursor, 5);
    }

    #[test]
    fn typing_inserts_chars_at_cursor() {
        let mut app = app();
        let (tx, _rx) = channel();
        app.input = "hllo".to_string();
        app.cursor = 1;

        app.handle_key(key(KeyCode::Char('e')), &tx);

        assert_eq!(app.input, "hello");
        assert_eq!(app.cursor, 2);
    }

    #[test]
    fn backspace_deletes_char_before_cursor() {
        let mut app = app();
        let (tx, _rx) = channel();
        app.input = "hxello".to_string();
        app.cursor = 2;

        app.handle_key(key(KeyCode::Backspace), &tx);

        assert_eq!(app.input, "hello");
        assert_eq!(app.cursor, 1);
    }

    #[test]
    fn backspace_at_start_is_noop() {
        let mut app = app();
        let (tx, _rx) = channel();
        app.input = "hello".to_string();
        app.cursor = 0;

        app.handle_key(key(KeyCode::Backspace), &tx);

        assert_eq!(app.input, "hello");
        assert_eq!(app.cursor, 0);
    }

    #[test]
    fn arrow_keys_walk_char_boundaries() {
        let mut app = app();
        let (tx, _rx) = channel();
        app.input = "ab".to_string();
        app.cursor = 2;

        app.handle_key(key(KeyCode::Left), &tx);
        assert_eq!(app.cursor, 1);
        app.handle_key(key(KeyCode::Left), &tx);
        assert_eq!(app.cursor, 0);
        app.handle_key(key(KeyCode::Left), &tx);
        assert_eq!(app.cursor, 0);

        app.handle_key(key(KeyCode::Right), &tx);
        assert_eq!(app.cursor, 1);
        app.handle_key(key(KeyCode::Right), &tx);
        assert_eq!(app.cursor, 2);
        app.handle_key(key(KeyCode::Right), &tx);
        assert_eq!(app.cursor, 2);
    }

    #[test]
    fn left_right_step_over_multibyte_char() {
        let mut app = app();
        let (tx, _rx) = channel();
        // 'é' is 2 bytes in UTF-8.
        app.input = "aéb".to_string();
        app.cursor = 0;

        app.handle_key(key(KeyCode::Right), &tx);
        assert_eq!(app.cursor, 1);
        app.handle_key(key(KeyCode::Right), &tx);
        assert_eq!(app.cursor, 3);
        app.handle_key(key(KeyCode::Left), &tx);
        assert_eq!(app.cursor, 1);
    }

    #[test]
    fn home_and_end_jump_to_edges() {
        let mut app = app();
        let (tx, _rx) = channel();
        app.input = "hello".to_string();
        app.cursor = 2;

        app.handle_key(key(KeyCode::Home), &tx);
        assert_eq!(app.cursor, 0);
        app.handle_key(key(KeyCode::End), &tx);
        assert_eq!(app.cursor, 5);
    }

    #[test]
    fn submit_resets_cursor_to_zero() {
        let mut app = app();
        let (tx, mut rx) = channel();
        app.input = "hi".to_string();
        app.cursor = 2;

        app.handle_key(key(KeyCode::Enter), &tx);

        let events = drain(&mut rx);
        assert!(matches!(&events[0], AppEvent::Submit(t) if t == "hi"));
        assert_eq!(app.cursor, 0);
        assert!(app.input.is_empty());
    }

    #[test]
    fn ctrl_c_quits_from_any_mode() {
        let mut app = app();
        let (tx, mut rx) = channel();
        app.mode = ViewMode::ModelPicker;

        app.handle_key(ctrl_key('c'), &tx);

        let events = drain(&mut rx);
        assert!(matches!(events.as_slice(), [AppEvent::Quit]));
    }

    #[test]
    fn auth_slash_command_opens_wizard() {
        let mut app = app();
        let (tx, _rx) = channel();
        app.input = "/auth".to_string();

        app.handle_key(key(KeyCode::Enter), &tx);

        assert!(matches!(app.mode, ViewMode::AuthWizard));
        assert!(matches!(app.auth_wizard.step, AuthStep::SelectProvider));
    }

    #[test]
    fn auth_wizard_provider_selection_bounded() {
        let mut app = app();
        let (tx, _rx) = channel();
        app.open_auth_wizard();

        app.handle_key(key(KeyCode::Down), &tx);
        assert_eq!(app.auth_wizard.provider_idx, 1);
        app.handle_key(key(KeyCode::Down), &tx);
        assert_eq!(app.auth_wizard.provider_idx, 2);
        app.handle_key(key(KeyCode::Down), &tx);
        assert_eq!(app.auth_wizard.provider_idx, 2, "should be bounded");
        app.handle_key(key(KeyCode::Home), &tx);
        assert_eq!(app.auth_wizard.provider_idx, 0);
        app.handle_key(key(KeyCode::End), &tx);
        assert_eq!(app.auth_wizard.provider_idx, 2);
    }

    #[test]
    fn auth_wizard_anthropic_skips_origin_step() {
        let mut app = app();
        let (tx, _rx) = channel();
        app.open_auth_wizard();
        // provider_idx 0 = Anthropic
        app.handle_key(key(KeyCode::Enter), &tx);

        assert!(matches!(app.auth_wizard.step, AuthStep::EnterApiKey));
    }

    #[test]
    fn auth_wizard_custom_goes_through_origin_step() {
        let mut app = app();
        let (tx, _rx) = channel();
        app.open_auth_wizard();
        // Navigate to Custom (idx 2)
        app.handle_key(key(KeyCode::End), &tx);
        app.handle_key(key(KeyCode::Enter), &tx);

        assert!(matches!(app.auth_wizard.step, AuthStep::EnterOrigin));

        // Type a valid origin
        for c in "https://api.together.ai".chars() {
            app.handle_key(key(KeyCode::Char(c)), &tx);
        }
        app.handle_key(key(KeyCode::Enter), &tx);

        assert!(matches!(app.auth_wizard.step, AuthStep::EnterApiKey));
        assert!(app.auth_wizard.error.is_none());
    }

    #[test]
    fn auth_wizard_origin_invalid_shows_error() {
        let mut app = app();
        let (tx, _rx) = channel();
        app.open_auth_wizard();
        app.handle_key(key(KeyCode::End), &tx);
        app.handle_key(key(KeyCode::Enter), &tx);

        // Type an origin with a path (invalid)
        for c in "https://example.com/v1".chars() {
            app.handle_key(key(KeyCode::Char(c)), &tx);
        }
        app.handle_key(key(KeyCode::Enter), &tx);

        assert!(matches!(app.auth_wizard.step, AuthStep::EnterOrigin));
        assert!(app.auth_wizard.error.is_some());
    }

    #[test]
    fn auth_wizard_empty_key_does_not_submit() {
        let mut app = app();
        let (tx, mut rx) = channel();
        app.open_auth_wizard();
        app.handle_key(key(KeyCode::Enter), &tx); // advance to EnterApiKey
        drain(&mut rx);

        app.handle_key(key(KeyCode::Enter), &tx); // try to submit with empty key

        assert!(drain(&mut rx).is_empty());
        assert!(matches!(app.mode, ViewMode::AuthWizard));
    }

    #[test]
    fn auth_wizard_submit_emits_event_and_closes() {
        let mut app = app();
        let (tx, mut rx) = channel();
        app.open_auth_wizard();
        // Anthropic, advance to key field
        app.handle_key(key(KeyCode::Enter), &tx);
        drain(&mut rx);

        for c in "sk-ant-abc123".chars() {
            app.handle_key(key(KeyCode::Char(c)), &tx);
        }
        app.handle_key(key(KeyCode::Enter), &tx);

        let events = drain(&mut rx);
        assert_eq!(events.len(), 1);
        match &events[0] {
            AppEvent::AuthSubmit { provider, origin, api_key } => {
                assert!(matches!(provider, crate::provider::Provider::Anthropic));
                assert!(origin.is_none());
                assert_eq!(api_key, "sk-ant-abc123");
            }
            other => panic!("expected AuthSubmit, got {other:?}"),
        }
        assert!(matches!(app.mode, ViewMode::Chat));
    }

    #[test]
    fn auth_wizard_esc_cancels_without_event() {
        let mut app = app();
        let (tx, mut rx) = channel();
        app.open_auth_wizard();

        app.handle_key(key(KeyCode::Esc), &tx);

        assert!(drain(&mut rx).is_empty());
        assert!(matches!(app.mode, ViewMode::Chat));
    }

    #[test]
    fn append_assistant_holds_partial_until_newline() {
        let mut app = app();
        app.begin_assistant();

        let lines = app.append_assistant("hello");
        assert!(lines.is_empty());

        // First flush emits BOT prefix + the committed content line.
        let lines = app.append_assistant(" world\n");
        assert_eq!(lines.len(), 2);

        let live = app.live.as_ref().unwrap();
        assert_eq!(live.raw, "hello world\n");
        assert_eq!(live.committed_bytes, "hello world\n".len());
        assert!(live.prefix_emitted);

        // Subsequent flush has no prefix.
        let lines = app.append_assistant("more\n");
        assert_eq!(lines.len(), 1);
    }

    #[test]
    fn finish_assistant_flushes_partial_and_persists_message() {
        let mut app = app();
        app.begin_assistant();
        app.append_assistant("partial");

        // Trailing flush emits prefix + 1 content line because nothing was
        // committed during streaming.
        let trailing = app.finish_assistant();
        assert_eq!(trailing.len(), 2);
        assert!(!app.streaming);
        assert_eq!(app.messages.len(), 1);
        assert_eq!(app.messages[0].content, "partial");
    }

    #[test]
    fn finish_assistant_drops_empty_response() {
        let mut app = app();
        app.begin_assistant();

        let trailing = app.finish_assistant();
        assert!(trailing.is_empty());
        assert!(app.messages.is_empty());
    }

    #[test]
    fn set_models_selects_current_model_and_reports_status() {
        let mut app = app();

        app.set_models(vec![model("alpha"), model("test-model"), model("zeta")]);

        assert_eq!(app.model_picker.selected, 1);
        assert!(!app.model_picker.loading);
        assert_eq!(
            app.status.as_ref().map(|status| status.text.as_str()),
            Some("loaded 3 chat models")
        );
    }

    #[test]
    fn model_picker_navigation_and_selection_are_bounded() {
        let mut app = app();
        let (tx, mut rx) = channel();
        app.set_models(vec![model("alpha"), model("beta"), model("gamma")]);
        app.input = "/model".to_string();
        app.handle_key(key(KeyCode::Enter), &tx);
        drain(&mut rx);

        app.handle_key(key(KeyCode::End), &tx);
        assert_eq!(app.model_picker.selected, 2);
        app.handle_key(key(KeyCode::Down), &tx);
        assert_eq!(app.model_picker.selected, 2);
        app.handle_key(key(KeyCode::Home), &tx);
        assert_eq!(app.model_picker.selected, 0);

        app.handle_key(key(KeyCode::Enter), &tx);
        let events = drain(&mut rx);
        assert!(matches!(events.as_slice(), [AppEvent::SelectModel(m)] if m == "alpha"));
        assert!(matches!(app.mode, ViewMode::Chat));
        assert_eq!(app.current_model, "alpha");
    }

    #[test]
    fn set_error_stops_streaming_and_clears_live_cell() {
        let mut app = app();
        app.begin_assistant();
        app.append_assistant("partial");

        app.set_error("boom".to_string());

        assert!(!app.streaming);
        assert!(app.live.is_none());
        let status = app.status.as_ref().unwrap();
        assert_eq!(status.text, "error: boom");
        assert!(matches!(status.kind, StatusKind::Error));
    }

    #[test]
    fn clear_for_new_session_resets_chat_state_and_keeps_model() {
        let mut app = app();
        app.messages.push(Message {
            role: "user".to_string(),
            content: "hello".to_string(),
        });
        app.input = "draft".to_string();
        app.streaming = true;
        app.mode = ViewMode::ModelPicker;
        app.model_picker.loading = true;
        app.current_model = "chosen-model".to_string();

        app.clear_for_new_session("new-session");

        assert!(app.messages.is_empty());
        assert!(app.input.is_empty());
        assert!(!app.streaming);
        assert!(matches!(app.mode, ViewMode::Chat));
        assert!(!app.model_picker.loading);
        assert_eq!(app.current_model, "chosen-model");
        assert_eq!(
            app.status.as_ref().map(|status| status.text.as_str()),
            Some("session: new-session · model: chosen-model")
        );
    }
}
