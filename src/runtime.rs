use std::time::{Duration, Instant};

use anyhow::Result;
use crossterm::event::{Event, EventStream, KeyEventKind};
use futures_util::StreamExt;
use ratatui::{layout::Rect, text::Line};
use tokio::time::{Instant as TokioInstant, Sleep, interval, sleep_until};

use crate::{
    api,
    app::{App, ViewMode},
    app_event::{AppEvent, AppEventSender},
    config::Config,
    logger,
    provider::Provider,
    session::{self, SessionState, SessionStore},
    state,
    terminal::{self, Tui},
    ui,
    user_config,
};

pub enum InitialView {
    Chat,
    AuthWizard,
}

pub async fn run(
    terminal: &mut Tui,
    client: reqwest::Client,
    mut config: Config,
    mut session_state: SessionState,
    session_store: SessionStore,
    initial_view: InitialView,
) -> Result<String> {
    config.model = session_state.model.clone();
    logger::info(format_args!(
        "runtime starting session={} model={}",
        session_state.id, config.model
    ));

    let mut app = App::with_messages(
        config.model.clone(),
        session_state.messages.clone(),
        Some(&session_state.id),
    );
    if matches!(initial_view, InitialView::AuthWizard) {
        app.open_auth_wizard();
    }
    let mut events = EventStream::new();
    let mut tick = interval(Duration::from_millis(80));
    let (tx, mut rx) = crate::app_event::channel();
    const MIN_FRAME: Duration = Duration::from_millis(16);
    let mut last_draw = Instant::now();
    let mut throttle_wakeup: Option<std::pin::Pin<Box<Sleep>>> = None;

    push_lines(
        terminal,
        ui::render_session_banner(&format!(
            "session: {} · model: {}",
            session_state.id, config.model
        )),
    )?;
    if !app.messages.is_empty() {
        replay_history(terminal, &app)?;
    }

    let mut prev_picker_open = is_picker_open(&app);
    let mut alt_saved: Option<Rect> = if prev_picker_open {
        Some(terminal::enter_fullscreen(terminal)?)
    } else {
        None
    };
    let mut persist_gate = PersistGate::new();
    terminal::with_sync_update(|| {
        reshape_for_input(terminal, &app)?;
        terminal.draw(|f| ui::render(&mut app, f))?;
        Ok(())
    })?;

    loop {
        tokio::select! {
            maybe_event = events.next() => {
                match maybe_event {
                    Some(Ok(Event::Key(key))) if key.kind == KeyEventKind::Press => {
                        app.handle_key(key, &tx);
                        app.mark_dirty();
                    }
                    Some(Ok(Event::Resize(_, _))) => {
                        tx.send(AppEvent::Resize);
                        app.mark_dirty();
                    }
                    Some(Ok(_)) => {}
                    Some(Err(e)) => {
                        logger::error(format_args!("terminal event error: {e}"));
                        persist_gate.flush(&session_store, &mut session_state, &app);
                        break;
                    }
                    None => {
                        logger::warn(format_args!("terminal event stream ended"));
                        persist_gate.flush(&session_store, &mut session_state, &app);
                        break;
                    }
                }
            }
            Some(ev) = rx.recv() => {
                if !handle_app_event(
                    ev,
                    &mut app,
                    terminal,
                    &tx,
                    &client,
                    &mut config,
                    &session_store,
                    &mut session_state,
                    &mut persist_gate,
                )? {
                    break;
                }
                app.mark_dirty();
            }
            _ = tick.tick() => {
                app.tick();
            }
            _ = async {
                match throttle_wakeup.as_mut() {
                    Some(s) => s.as_mut().await,
                    None => std::future::pending::<()>().await,
                }
            }, if throttle_wakeup.is_some() => {
                throttle_wakeup = None;
            }
        }

        let now_picker_open = is_picker_open(&app);
        if now_picker_open != prev_picker_open {
            if now_picker_open {
                alt_saved = Some(terminal::enter_fullscreen(terminal)?);
            } else if let Some(saved) = alt_saved.take() {
                terminal::leave_fullscreen(terminal, saved)?;
            }
            prev_picker_open = now_picker_open;
            app.mark_dirty();
        }

        if app.take_dirty() {
            let elapsed = last_draw.elapsed();
            if elapsed >= MIN_FRAME {
                terminal::with_sync_update(|| {
                    reshape_for_input(terminal, &app)?;
                    terminal.draw(|f| ui::render(&mut app, f))?;
                    Ok(())
                })?;
                last_draw = Instant::now();
            } else {
                // Frame too soon; reschedule the draw for after the budget.
                app.mark_dirty();
                let deadline = TokioInstant::now() + (MIN_FRAME - elapsed);
                throttle_wakeup = Some(Box::pin(sleep_until(deadline)));
            }
        }
    }

    if let Some(saved) = alt_saved.take() {
        terminal::leave_fullscreen(terminal, saved).ok();
    }

    logger::info(format_args!("runtime stopped"));
    Ok(session_state.id)
}

#[allow(clippy::too_many_arguments)]
fn handle_app_event(
    event: AppEvent,
    app: &mut App,
    terminal: &mut Tui,
    tx: &AppEventSender,
    client: &reqwest::Client,
    config: &mut Config,
    session_store: &SessionStore,
    session_state: &mut SessionState,
    persist_gate: &mut PersistGate,
) -> Result<bool> {
    match event {
        AppEvent::Quit => {
            logger::info(format_args!("quit requested"));
            persist_gate.flush(session_store, session_state, app);
            return Ok(false);
        }
        AppEvent::Clear => {
            logger::info(format_args!("clear requested model={}", app.current_model));
            persist_gate.flush(session_store, session_state, app);
            match session_store.create(&app.current_model) {
                Ok(new_state) => {
                    logger::info(format_args!("new session created id={}", new_state.id));
                    *session_state = new_state;
                    app.clear_for_new_session(&session_state.id);
                    push_lines(
                        terminal,
                        ui::render_session_banner(&format!(
                            "session: {} · model: {}",
                            session_state.id, config.model
                        )),
                    )?;
                }
                Err(error) => {
                    logger::error(format_args!("session create error: {error:#}"));
                    app.set_error(format!("{error:#}"));
                }
            }
        }
        AppEvent::Submit(text) => {
            logger::info(format_args!(
                "submitting prompt bytes={} model={}",
                text.len(),
                config.model
            ));
            push_lines(terminal, ui::render_user_message(&text))?;
            app.push_user(text);
            app.begin_assistant();
            persist_gate.flush(session_store, session_state, app);

            let history = app.messages.clone();
            let tx2 = tx.clone();
            let client2 = client.clone();
            let cfg = config.clone();
            tokio::spawn(async move {
                api::stream_chat(client2, cfg, history, tx2).await;
            });
        }
        AppEvent::LoadModels => {
            logger::info(format_args!("loading models"));
            let tx2 = tx.clone();
            let client2 = client.clone();
            let cfg = config.clone();
            tokio::spawn(async move {
                api::load_models(client2, cfg, tx2).await;
            });
        }
        AppEvent::SelectModel(model) => {
            logger::info(format_args!("model selected {model}"));
            config.model = model.clone();
            persist_gate.flush(session_store, session_state, app);
            if let Err(error) = user_config::persist_active_model(&model) {
                logger::error(format_args!("user config save error: {error:#}"));
                app.set_error(format!("config save failed: {error:#}"));
            }
        }
        AppEvent::StreamToken(t) => {
            let lines = app.append_assistant(&t);
            if !lines.is_empty() {
                push_lines(terminal, lines)?;
            }
            persist_gate.mark();
            persist_gate.maybe_flush(session_store, session_state, app);
        }
        AppEvent::StreamError(e) => {
            logger::error(format_args!("stream error: {e}"));
            push_lines(terminal, ui::render_error(&e))?;
            app.set_error(e);
            persist_gate.flush(session_store, session_state, app);
        }
        AppEvent::StreamDone => {
            logger::info(format_args!("stream done"));
            let mut trailing = app.finish_assistant();
            trailing.extend(ui::render_assistant_trailer());
            push_lines(terminal, trailing)?;
            persist_gate.flush(session_store, session_state, app);
        }
        AppEvent::ModelsLoaded(models) => {
            logger::info(format_args!("loaded {} models", models.len()));
            app.set_models(models);
        }
        AppEvent::ModelsError(e) => {
            logger::error(format_args!("model load error: {e}"));
            app.set_model_load_error(e);
        }
        AppEvent::AuthSubmit { provider, origin, api_key } => {
            let profile_name = match (provider, origin.as_ref()) {
                (Provider::Anthropic, _)     => "anthropic",
                (Provider::OpenRouter, None) => "openrouter",
                (Provider::OpenRouter, _)    => "custom",
            };
            match user_config::upsert_auth_profile(profile_name, provider, api_key, origin) {
                Ok(()) => {
                    if let Err(e) = state::mark_auth_completed() {
                        logger::warn(format_args!("state save error: {e:#}"));
                    }
                    match Config::from_env() {
                        Ok(new_config) => {
                            *config = new_config;
                            app.current_model = config.model.clone();
                            app.models.clear();
                            let msg = format!(
                                "auth saved · profile={profile_name} · model: {}",
                                config.model
                            );
                            app.set_status_info(msg.clone());
                            push_lines(terminal, ui::render_session_banner(&msg))?;
                        }
                        Err(e) => {
                            app.set_error(format!("config reload failed: {e:#}"));
                        }
                    }
                }
                Err(e) => {
                    app.set_error(format!("auth save failed: {e:#}"));
                }
            }
        }
        AppEvent::Resize => {}
    }
    Ok(true)
}

fn push_lines(terminal: &mut Tui, lines: Vec<Line<'static>>) -> Result<()> {
    if lines.is_empty() {
        return Ok(());
    }
    let lines: Vec<Line> = lines.into_iter().map(Into::into).collect();
    let mode = crate::insert_history::InsertHistoryMode::new(is_zellij());
    terminal::with_sync_update(|| {
        crate::insert_history::insert_history_lines(terminal, lines, mode)
            .map_err(anyhow::Error::from)
    })
}

fn is_zellij() -> bool {
    std::env::var_os("ZELLIJ").is_some()
        || std::env::var("TERM_PROGRAM")
            .map(|v| v.eq_ignore_ascii_case("zellij"))
            .unwrap_or(false)
}

fn replay_history(terminal: &mut Tui, app: &App) -> Result<()> {
    for msg in &app.messages {
        let lines = match msg.role.as_str() {
            "user" => ui::render_user_message(&msg.content),
            "assistant" => {
                let mut lines = ui::render_assistant_chunk(&msg.content, true);
                lines.extend(ui::render_assistant_trailer());
                lines
            }
            _ => continue,
        };
        push_lines(terminal, lines)?;
    }
    Ok(())
}

fn is_picker_open(app: &App) -> bool {
    matches!(app.mode, ViewMode::ModelPicker | ViewMode::AuthWizard)
}

fn reshape_for_input(terminal: &mut crate::terminal::Tui, app: &App) -> Result<()> {
    if matches!(app.mode, ViewMode::ModelPicker | ViewMode::AuthWizard) {
        return Ok(());
    }
    let screen = terminal.size()?;
    let text_width = ui::input_text_width(screen.width);
    let input_rows = app.input.desired_height(text_width);
    let desired = input_rows.saturating_add(ui::INPUT_CHROME_ROWS);
    terminal::reshape_viewport(terminal, desired)
}

fn persist_session(store: &SessionStore, state: &mut SessionState, app: &App) {
    state.updated_at = session::timestamp();
    state.model = app.current_model.clone();
    state.messages = app.messages.clone();

    if let Err(error) = store.save(state) {
        logger::error(format_args!("session save error: {error:#}"));
    }
}

struct PersistGate {
    last_flush: Instant,
    pending: bool,
}

impl PersistGate {
    fn new() -> Self {
        Self {
            last_flush: Instant::now(),
            pending: false,
        }
    }

    fn flush(&mut self, store: &SessionStore, state: &mut SessionState, app: &App) {
        persist_session(store, state, app);
        self.last_flush = Instant::now();
        self.pending = false;
    }

    fn maybe_flush(&mut self, store: &SessionStore, state: &mut SessionState, app: &App) {
        if self.pending && self.last_flush.elapsed() >= Duration::from_secs(1) {
            self.flush(store, state, app);
        }
    }

    fn mark(&mut self) {
        self.pending = true;
    }
}
