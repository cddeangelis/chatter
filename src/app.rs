use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::text::Line;

use crate::{
    api::{Message, ModelInfo},
    app_event::{AppEvent, AppEventSender},
    command_popup::CommandPopup,
    commands::{CommandError, SlashCommand, parse_slash_command},
    provider::Provider,
    textarea::{TextArea, TextAreaState},
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
    pub filter: String,
}

impl ModelPicker {
    /// Indices into `models` that match the current filter (case-insensitive
    /// substring on id and name). Empty filter returns every index.
    pub fn matches(&self, models: &[ModelInfo]) -> Vec<usize> {
        if self.filter.is_empty() {
            return (0..models.len()).collect();
        }
        let needle = self.filter.to_lowercase();
        models
            .iter()
            .enumerate()
            .filter(|(_, m)| {
                m.id.to_lowercase().contains(&needle)
                    || m.name
                        .as_deref()
                        .is_some_and(|n| n.to_lowercase().contains(&needle))
            })
            .map(|(i, _)| i)
            .collect()
    }
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
    pub input: TextArea,
    pub input_state: TextAreaState,
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
    pub command_popup: Option<CommandPopup>,
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
            input: TextArea::new(),
            input_state: TextAreaState::default(),
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
                filter: String::new(),
            },
            auth_wizard: AuthWizard::default(),
            live: None,
            dirty: true,
            command_popup: None,
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
        let shift = key.modifiers.contains(KeyModifiers::SHIFT);

        if self.command_popup.is_some() {
            match key.code {
                KeyCode::Up => {
                    self.command_popup.as_mut().unwrap().move_up();
                    self.mark_dirty();
                    return;
                }
                KeyCode::Down => {
                    self.command_popup.as_mut().unwrap().move_down();
                    self.mark_dirty();
                    return;
                }
                KeyCode::Esc => {
                    self.command_popup = None;
                    self.mark_dirty();
                    return;
                }
                KeyCode::Tab => {
                    if let Some(cmd) = self.command_popup.as_ref().and_then(|p| p.selected()) {
                        let autocomplete = format!("/{} ", cmd.command());
                        self.input.clear();
                        self.input.insert_str(&autocomplete);
                        self.input_state = TextAreaState::default();
                        self.sync_command_popup();
                    }
                    return;
                }
                KeyCode::Enter if !shift => {
                    if let Some(cmd) = self.command_popup.as_ref().and_then(|p| p.selected()) {
                        self.clear_input();
                        self.command_popup = None;
                        self.dispatch_slash_command(cmd, tx);
                        return;
                    }
                }
                _ => {}
            }
        }

        match key.code {
            KeyCode::Esc => tx.send(AppEvent::Quit),
            KeyCode::Enter if !self.streaming && !shift => {
                self.submit_input(tx);
            }
            _ if !self.streaming => {
                self.input.input(key);
                self.sync_command_popup();
            }
            _ => {}
        }
    }

    fn sync_command_popup(&mut self) {
        if self.streaming {
            self.command_popup = None;
            return;
        }
        let first_line = self.input.text().lines().next().unwrap_or("").to_string();
        if first_line.starts_with('/') {
            let popup = self.command_popup.get_or_insert_with(CommandPopup::new);
            popup.on_text_change(&first_line);
        } else {
            self.command_popup = None;
        }
        self.mark_dirty();
    }

    fn submit_input(&mut self, tx: &AppEventSender) {
        let text = self.input.text().trim().to_string();
        if text.is_empty() {
            return;
        }
        self.command_popup = None;
        if let Some(command_text) = text.strip_prefix('/') {
            self.clear_input();
            self.handle_slash_command(command_text, tx);
            return;
        }
        self.clear_input();
        tx.send(AppEvent::Submit(text));
    }

    pub fn clear_input(&mut self) {
        self.input.clear();
        self.input_state = TextAreaState::default();
    }

    fn handle_slash_command(&mut self, command_text: &str, tx: &AppEventSender) {
        match parse_slash_command(command_text) {
            Ok(cmd) => self.dispatch_slash_command(cmd, tx),
            Err(CommandError::Unknown(name)) => {
                self.status = Some(Status::info(format!("unknown command: /{name}")));
            }
            Err(CommandError::Empty) => {
                let hint: Vec<&str> = SlashCommand::all().iter().map(|c| c.command()).collect();
                self.status = Some(Status::info(format!(
                    "type /{}",
                    hint.join(", /")
                )));
            }
        }
    }

    fn dispatch_slash_command(&mut self, cmd: SlashCommand, tx: &AppEventSender) {
        match cmd {
            SlashCommand::Auth => self.open_auth_wizard(),
            SlashCommand::Clear => tx.send(AppEvent::Clear),
            SlashCommand::Exit => tx.send(AppEvent::Quit),
            SlashCommand::Model => {
                self.open_model_picker();
                if self.models.is_empty() {
                    self.model_picker.loading = true;
                    tx.send(AppEvent::LoadModels);
                }
            }
        }
    }

    fn handle_model_picker_key(&mut self, key: KeyEvent, tx: &AppEventSender) {
        match key.code {
            KeyCode::Esc => {
                self.close_model_picker();
            }
            KeyCode::Up => {
                self.move_model_selection(-1);
            }
            KeyCode::Down => {
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
                let last = self.model_picker.matches(&self.models).len().saturating_sub(1);
                self.model_picker.selected = last;
                self.keep_selected_model_visible();
            }
            KeyCode::Enter => {
                let matches = self.model_picker.matches(&self.models);
                if let Some(&idx) = matches.get(self.model_picker.selected) {
                    let model = self.models[idx].id.clone();
                    self.set_current_model(model.clone());
                    self.close_model_picker();
                    tx.send(AppEvent::SelectModel(model));
                }
            }
            KeyCode::Backspace => {
                if self.model_picker.filter.pop().is_some() {
                    self.refresh_model_filter();
                }
            }
            KeyCode::Char(c) if !key.modifiers.contains(KeyModifiers::CONTROL) => {
                if (c as u32) >= 32 && (c as u32) != 127 {
                    self.model_picker.filter.push(c);
                    self.refresh_model_filter();
                }
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
        self.model_picker.filter.clear();
        self.model_picker.scroll = 0;
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
        let matches = self.model_picker.matches(&self.models);
        let position = matches
            .iter()
            .position(|&i| self.models[i].id == self.current_model);
        self.model_picker.selected = position.unwrap_or(0);
    }

    fn move_model_selection(&mut self, delta: isize) {
        let len = self.model_picker.matches(&self.models).len();
        if len == 0 {
            return;
        }
        let max = len - 1;
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

    fn refresh_model_filter(&mut self) {
        let len = self.model_picker.matches(&self.models).len();
        if len == 0 {
            self.model_picker.selected = 0;
            self.model_picker.scroll = 0;
        } else {
            self.model_picker.selected = self.model_picker.selected.min(len - 1);
            self.model_picker.scroll = self.model_picker.scroll.min(self.model_picker.selected);
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

    fn shift_key(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::SHIFT)
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
        app.input.insert_str("  hello  ");

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
        app.input.insert_str("   ");

        app.handle_key(key(KeyCode::Enter), &tx);

        assert!(drain(&mut rx).is_empty());
        assert_eq!(app.input.text(), "   ");
    }

    #[test]
    fn handle_key_routes_exit_command_to_quit() {
        let mut app = app();
        let (tx, mut rx) = channel();
        app.input.insert_str("/quit");

        app.handle_key(key(KeyCode::Enter), &tx);

        let events = drain(&mut rx);
        assert!(matches!(events.as_slice(), [AppEvent::Quit]));
        assert!(app.input.is_empty());
    }

    #[test]
    fn handle_key_routes_clear_command_to_clear_event() {
        let mut app = app();
        let (tx, mut rx) = channel();
        app.input.insert_str("/clear");

        app.handle_key(key(KeyCode::Enter), &tx);

        let events = drain(&mut rx);
        assert!(matches!(events.as_slice(), [AppEvent::Clear]));
    }

    #[test]
    fn handle_key_opens_model_picker_and_loads_when_empty() {
        let mut app = app();
        let (tx, mut rx) = channel();
        app.input.insert_str("/model");

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
        app.input.insert_str("/model");

        app.handle_key(key(KeyCode::Enter), &tx);

        assert!(drain(&mut rx).is_empty());
        assert!(matches!(app.mode, ViewMode::ModelPicker));
        assert!(!app.model_picker.loading);
    }

    #[test]
    fn streaming_blocks_text_editing_and_submit() {
        let mut app = app();
        let (tx, mut rx) = channel();
        app.input.insert_str("hello");
        app.input.set_cursor(app.input.text().len());
        app.streaming = true;

        app.handle_key(key(KeyCode::Char('!')), &tx);
        app.handle_key(key(KeyCode::Enter), &tx);
        app.handle_key(key(KeyCode::Left), &tx);
        app.handle_key(key(KeyCode::Home), &tx);

        assert!(drain(&mut rx).is_empty());
        assert_eq!(app.input.text(), "hello");
        assert_eq!(app.input.cursor(), 5);
    }

    #[test]
    fn typing_inserts_chars_at_cursor() {
        let mut app = app();
        let (tx, _rx) = channel();
        app.input.insert_str("hllo");
        app.input.set_cursor(1);

        app.handle_key(key(KeyCode::Char('e')), &tx);

        assert_eq!(app.input.text(), "hello");
        assert_eq!(app.input.cursor(), 2);
    }

    #[test]
    fn backspace_deletes_char_before_cursor() {
        let mut app = app();
        let (tx, _rx) = channel();
        app.input.insert_str("hxello");
        app.input.set_cursor(2);

        app.handle_key(key(KeyCode::Backspace), &tx);

        assert_eq!(app.input.text(), "hello");
        assert_eq!(app.input.cursor(), 1);
    }

    #[test]
    fn backspace_at_start_is_noop() {
        let mut app = app();
        let (tx, _rx) = channel();
        app.input.insert_str("hello");
        app.input.set_cursor(0);

        app.handle_key(key(KeyCode::Backspace), &tx);

        assert_eq!(app.input.text(), "hello");
        assert_eq!(app.input.cursor(), 0);
    }

    #[test]
    fn arrow_keys_walk_char_boundaries() {
        let mut app = app();
        let (tx, _rx) = channel();
        app.input.insert_str("ab");
        app.input.set_cursor(2);

        app.handle_key(key(KeyCode::Left), &tx);
        assert_eq!(app.input.cursor(), 1);
        app.handle_key(key(KeyCode::Left), &tx);
        assert_eq!(app.input.cursor(), 0);
        app.handle_key(key(KeyCode::Left), &tx);
        assert_eq!(app.input.cursor(), 0);

        app.handle_key(key(KeyCode::Right), &tx);
        assert_eq!(app.input.cursor(), 1);
        app.handle_key(key(KeyCode::Right), &tx);
        assert_eq!(app.input.cursor(), 2);
        app.handle_key(key(KeyCode::Right), &tx);
        assert_eq!(app.input.cursor(), 2);
    }

    #[test]
    fn left_right_step_over_multibyte_char() {
        let mut app = app();
        let (tx, _rx) = channel();
        // 'é' is 2 bytes in UTF-8.
        app.input.insert_str("aéb");
        app.input.set_cursor(0);

        app.handle_key(key(KeyCode::Right), &tx);
        assert_eq!(app.input.cursor(), 1);
        app.handle_key(key(KeyCode::Right), &tx);
        assert_eq!(app.input.cursor(), 3);
        app.handle_key(key(KeyCode::Left), &tx);
        assert_eq!(app.input.cursor(), 1);
    }

    #[test]
    fn home_and_end_jump_to_edges() {
        let mut app = app();
        let (tx, _rx) = channel();
        app.input.insert_str("hello");
        app.input.set_cursor(2);

        app.handle_key(key(KeyCode::Home), &tx);
        assert_eq!(app.input.cursor(), 0);
        app.handle_key(key(KeyCode::End), &tx);
        assert_eq!(app.input.cursor(), 5);
    }

    #[test]
    fn submit_resets_cursor_to_zero() {
        let mut app = app();
        let (tx, mut rx) = channel();
        app.input.insert_str("hi");
        app.input.set_cursor(2);

        app.handle_key(key(KeyCode::Enter), &tx);

        let events = drain(&mut rx);
        assert!(matches!(&events[0], AppEvent::Submit(t) if t == "hi"));
        assert_eq!(app.input.cursor(), 0);
        assert!(app.input.is_empty());
    }

    #[test]
    fn shift_enter_inserts_newline_without_submitting() {
        let mut app = app();
        let (tx, mut rx) = channel();
        app.input.insert_str("hello");
        app.input.set_cursor(app.input.text().len());

        app.handle_key(shift_key(KeyCode::Enter), &tx);
        app.handle_key(key(KeyCode::Char('w')), &tx);

        assert!(drain(&mut rx).is_empty());
        assert_eq!(app.input.text(), "hello\nw");
    }

    #[test]
    fn plain_enter_submits_multiline_text_intact() {
        let mut app = app();
        let (tx, mut rx) = channel();
        app.input.insert_str("foo");
        app.input.set_cursor(app.input.text().len());
        app.handle_key(shift_key(KeyCode::Enter), &tx);
        app.handle_key(key(KeyCode::Char('b')), &tx);
        app.handle_key(key(KeyCode::Char('a')), &tx);
        app.handle_key(key(KeyCode::Char('r')), &tx);

        app.handle_key(key(KeyCode::Enter), &tx);

        let events = drain(&mut rx);
        assert!(matches!(&events[0], AppEvent::Submit(t) if t == "foo\nbar"));
        assert!(app.input.is_empty());
    }

    #[test]
    fn up_arrow_moves_cursor_across_explicit_newline() {
        let mut app = app();
        let (tx, _rx) = channel();
        app.input.insert_str("foo");
        app.input.set_cursor(app.input.text().len());
        app.handle_key(shift_key(KeyCode::Enter), &tx);
        app.handle_key(key(KeyCode::Char('b')), &tx);
        app.handle_key(key(KeyCode::Char('a')), &tx);
        app.handle_key(key(KeyCode::Char('r')), &tx);
        // cursor is at end of "foo\nbar" (position 7)
        assert_eq!(app.input.cursor(), 7);

        app.handle_key(key(KeyCode::Up), &tx);
        // Should land at end of "foo" (position 3) — same display column as "bar".
        assert_eq!(app.input.cursor(), 3);
    }

    #[test]
    fn streaming_blocks_shift_enter_newline() {
        let mut app = app();
        let (tx, mut rx) = channel();
        app.input.insert_str("hello");
        app.input.set_cursor(app.input.text().len());
        app.streaming = true;

        app.handle_key(shift_key(KeyCode::Enter), &tx);

        assert!(drain(&mut rx).is_empty());
        assert_eq!(app.input.text(), "hello");
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
        app.input.insert_str("/auth");

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
        app.input.insert_str("/model");
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
        app.input.insert_str("draft");
        app.streaming = true;
        app.mode = ViewMode::ModelPicker;
        app.model_picker.loading = true;
        app.current_model = "chosen-model".to_string();

        app.clear_for_new_session("new-session");

        assert!(app.messages.is_empty());
        assert!(app.input.text().is_empty());
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
