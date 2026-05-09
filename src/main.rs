use anyhow::Result;

use chatter::{
    api,
    config::Config,
    logger,
    runtime::{self, InitialView},
    session::{self, SessionCommand, StartupCommand},
    state::AppState,
    terminal,
};

#[tokio::main]
async fn main() -> Result<()> {
    let startup = session::parse_args(std::env::args())?;
    let log_path = session::startup_log_path()?;
    logger::init(&log_path)?;
    logger::info(format_args!("chatter starting log={log_path}"));

    let app_state = AppState::load().unwrap_or_else(|e| {
        logger::warn(format_args!("state load failed: {e:#}"));
        AppState::default()
    });

    let initial_view = match &startup {
        StartupCommand::Auth => InitialView::AuthWizard,
        StartupCommand::Chat(_) if !app_state.auth_completed => InitialView::AuthWizard,
        StartupCommand::Chat(_) => InitialView::Chat,
    };

    let session_command = match startup {
        StartupCommand::Auth    => SessionCommand::New,
        StartupCommand::Chat(c) => c,
    };

    let config = Config::from_env()?;
    let session_store = session::SessionStore::default()?;
    let session_state = session_store.load_or_create(&session_command, &config.model)?;
    let session_id = session_state.id.clone();
    logger::info(format_args!("session loaded id={}", session_state.id));

    let client = api::build_client()?;
    let mut terminal = terminal::setup()?;
    let result = runtime::run(
        &mut terminal,
        client,
        config,
        session_state,
        session_store,
        initial_view,
    )
    .await;
    terminal::restore(&mut terminal);
    let resume_session_id = result.as_ref().map_or(session_id.as_str(), String::as_str);
    println!("resume with:\n\tchatter resume {resume_session_id}");

    if let Err(error) = &result {
        logger::error(format_args!("fatal error: {error:#}"));
    }
    logger::info(format_args!("chatter exiting"));

    result.map(|_| ())
}
