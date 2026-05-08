use std::{
    fs,
    path::{Path, PathBuf},
    time::{SystemTime, UNIX_EPOCH},
};

use anyhow::{Context, Result, anyhow, bail};
use serde::{Deserialize, Serialize};

use crate::api::Message;

#[derive(Clone, Debug)]
pub enum SessionCommand {
    New,
    Resume(String),
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SessionState {
    pub id: String,
    pub created_at: u64,
    pub updated_at: u64,
    pub model: String,
    pub messages: Vec<Message>,
}

pub struct SessionStore {
    dir: PathBuf,
}

impl SessionStore {
    pub fn default() -> Result<Self> {
        Ok(Self {
            dir: config_dir()?.join("sessions"),
        })
    }

    pub fn load_or_create(
        &self,
        command: &SessionCommand,
        default_model: &str,
    ) -> Result<SessionState> {
        fs::create_dir_all(&self.dir).with_context(|| format!("create {}", self.dir.display()))?;

        match command {
            SessionCommand::New => self.create(default_model),
            SessionCommand::Resume(id) => self.load(id),
        }
    }

    pub fn create(&self, default_model: &str) -> Result<SessionState> {
        let now = timestamp();
        let state = SessionState {
            id: generate_uuid()?,
            created_at: now,
            updated_at: now,
            model: default_model.to_string(),
            messages: Vec::new(),
        };
        self.save(&state)?;
        Ok(state)
    }

    pub fn save(&self, state: &SessionState) -> Result<()> {
        fs::create_dir_all(&self.dir).with_context(|| format!("create {}", self.dir.display()))?;
        let path = self.path_for(&state.id)?;
        let json = serde_json::to_string_pretty(state).context("encode session json")?;
        fs::write(&path, json).with_context(|| format!("write {}", path.display()))
    }

    fn load(&self, id: &str) -> Result<SessionState> {
        validate_uuid(id)?;
        let path = self.path_for(id)?;
        let json = fs::read_to_string(&path).with_context(|| format!("read {}", path.display()))?;
        let mut state: SessionState =
            serde_json::from_str(&json).with_context(|| format!("decode {}", path.display()))?;
        if state.id != id {
            bail!("session id mismatch in {}", path.display());
        }
        state.updated_at = timestamp();
        Ok(state)
    }

    fn path_for(&self, id: &str) -> Result<PathBuf> {
        validate_uuid(id)?;
        Ok(self.dir.join(format!("{id}.json")))
    }
}

pub fn parse_args<I>(args: I) -> Result<SessionCommand>
where
    I: IntoIterator<Item = String>,
{
    let args = args.into_iter().skip(1).collect::<Vec<_>>();
    match args.as_slice() {
        [] => Ok(SessionCommand::New),
        [cmd, id] if cmd == "resume" => {
            validate_uuid(id)?;
            Ok(SessionCommand::Resume(id.clone()))
        }
        _ => Err(anyhow!("usage: chatter [resume <session-uuid>]")),
    }
}

pub fn startup_log_path() -> Result<String> {
    let dir = home_dir()?.join(".cache").join("chatter");
    fs::create_dir_all(&dir).with_context(|| format!("create {}", dir.display()))?;
    let pid = std::process::id();
    Ok(dir
        .join(format!("chatter-{}-{pid}.log", timestamp()))
        .display()
        .to_string())
}

pub fn timestamp() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .unwrap_or_default()
}

fn generate_uuid() -> Result<String> {
    Ok(uuid::Uuid::new_v4().to_string())
}

fn validate_uuid(id: &str) -> Result<()> {
    let valid = id.len() == 36
        && id.char_indices().all(|(idx, ch)| match idx {
            8 | 13 | 18 | 23 => ch == '-',
            _ => ch.is_ascii_hexdigit(),
        });

    if valid {
        Ok(())
    } else {
        bail!("invalid session uuid: {id}")
    }
}

pub(crate) fn config_dir() -> Result<PathBuf> {
    if let Some(value) = std::env::var_os("CHATTER_CONFIG_DIR") {
        if !value.is_empty() {
            return Ok(PathBuf::from(value));
        }
    }
    Ok(home_dir()?.join(".chatter"))
}

fn home_dir() -> Result<PathBuf> {
    std::env::var_os("HOME")
        .map(PathBuf::from)
        .filter(|path| !path.as_os_str().is_empty())
        .filter(|path| Path::new(path).is_absolute())
        .ok_or_else(|| anyhow!("HOME is not set to an absolute path"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;
    use std::sync::atomic::{AtomicUsize, Ordering};

    static NEXT_TEST_DIR: AtomicUsize = AtomicUsize::new(0);
    static ENV_LOCK: Mutex<()> = Mutex::new(());

    const ID: &str = "123e4567-e89b-12d3-a456-426614174000";
    const OTHER_ID: &str = "123e4567-e89b-12d3-a456-426614174001";

    fn args(values: &[&str]) -> Vec<String> {
        values.iter().map(|value| value.to_string()).collect()
    }

    fn store() -> SessionStore {
        let unique = NEXT_TEST_DIR.fetch_add(1, Ordering::Relaxed);
        let dir = std::env::temp_dir().join(format!(
            "chatter-session-test-{}-{unique}",
            std::process::id()
        ));
        SessionStore { dir }
    }

    #[test]
    fn parse_args_defaults_to_new_session() {
        assert!(matches!(
            parse_args(args(&["chatter"])).unwrap(),
            SessionCommand::New
        ));
    }

    #[test]
    fn parse_args_accepts_valid_resume_id() {
        match parse_args(args(&["chatter", "resume", ID])).unwrap() {
            SessionCommand::Resume(id) => assert_eq!(id, ID),
            SessionCommand::New => panic!("expected resume command"),
        }
    }

    #[test]
    fn parse_args_rejects_invalid_forms() {
        assert!(parse_args(args(&["chatter", "resume", "bad"])).is_err());
        assert!(parse_args(args(&["chatter", "resume"])).is_err());
        assert!(parse_args(args(&["chatter", "new"])).is_err());
    }

    #[test]
    fn validate_uuid_accepts_expected_shape_only() {
        assert!(validate_uuid(ID).is_ok());
        assert!(validate_uuid("123e4567e89b12d3a456426614174000").is_err());
        assert!(validate_uuid("123e4567-e89b-12d3-a456-42661417400z").is_err());
        assert!(validate_uuid("../123e4567-e89b-12d3-a456-426614174000").is_err());
    }

    #[test]
    fn save_and_load_round_trip_session_state() {
        let store = store();
        let state = SessionState {
            id: ID.to_string(),
            created_at: 10,
            updated_at: 20,
            model: "model-a".to_string(),
            messages: vec![Message {
                role: "user".to_string(),
                content: "hello".to_string(),
            }],
        };

        store.save(&state).unwrap();
        let loaded = store.load(ID).unwrap();

        assert_eq!(loaded.id, ID);
        assert_eq!(loaded.created_at, 10);
        assert!(loaded.updated_at >= 20);
        assert_eq!(loaded.model, "model-a");
        assert_eq!(loaded.messages.len(), 1);
        assert_eq!(loaded.messages[0].content, "hello");

        let _ = fs::remove_dir_all(&store.dir);
    }

    #[test]
    fn load_rejects_file_with_mismatched_session_id() {
        let store = store();
        fs::create_dir_all(&store.dir).unwrap();
        let path = store.path_for(ID).unwrap();
        let state = SessionState {
            id: OTHER_ID.to_string(),
            created_at: 10,
            updated_at: 20,
            model: "model-a".to_string(),
            messages: Vec::new(),
        };
        fs::write(path, serde_json::to_string(&state).unwrap()).unwrap();

        assert!(store.load(ID).is_err());

        let _ = fs::remove_dir_all(&store.dir);
    }

    #[test]
    fn config_dir_uses_env_override_when_set() {
        let _guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let prev = std::env::var_os("CHATTER_CONFIG_DIR");
        unsafe { std::env::set_var("CHATTER_CONFIG_DIR", "/tmp/custom-chatter-dir") };

        assert_eq!(
            config_dir().unwrap(),
            PathBuf::from("/tmp/custom-chatter-dir")
        );

        match prev {
            Some(v) => unsafe { std::env::set_var("CHATTER_CONFIG_DIR", v) },
            None => unsafe { std::env::remove_var("CHATTER_CONFIG_DIR") },
        }
    }

    #[test]
    fn config_dir_falls_back_to_home_dot_chatter_when_unset() {
        let _guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let prev = std::env::var_os("CHATTER_CONFIG_DIR");
        unsafe { std::env::remove_var("CHATTER_CONFIG_DIR") };

        let dir = config_dir().unwrap();
        assert_eq!(dir, home_dir().unwrap().join(".chatter"));

        if let Some(v) = prev {
            unsafe { std::env::set_var("CHATTER_CONFIG_DIR", v) };
        }
    }

    #[test]
    fn config_dir_treats_empty_env_var_as_unset() {
        let _guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let prev = std::env::var_os("CHATTER_CONFIG_DIR");
        unsafe { std::env::set_var("CHATTER_CONFIG_DIR", "") };

        let dir = config_dir().unwrap();
        assert_eq!(dir, home_dir().unwrap().join(".chatter"));

        match prev {
            Some(v) => unsafe { std::env::set_var("CHATTER_CONFIG_DIR", v) },
            None => unsafe { std::env::remove_var("CHATTER_CONFIG_DIR") },
        }
    }

    #[test]
    fn load_or_create_creates_new_session_with_default_model() {
        let store = store();

        let state = store
            .load_or_create(&SessionCommand::New, "default-model")
            .unwrap();

        assert!(validate_uuid(&state.id).is_ok());
        assert_eq!(state.model, "default-model");
        assert!(state.messages.is_empty());
        assert!(store.path_for(&state.id).unwrap().exists());

        let _ = fs::remove_dir_all(&store.dir);
    }
}
