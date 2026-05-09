use std::{
    fs,
    io::ErrorKind,
    path::{Path, PathBuf},
};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

use crate::user_config::config_dir;

#[derive(Default, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AppState {
    #[serde(default)]
    pub auth_completed: bool,
}

impl AppState {
    pub fn load() -> Result<Self> {
        Self::load_from(&path()?)
    }

    pub fn load_from(path: &Path) -> Result<Self> {
        match fs::read_to_string(path) {
            Ok(text) => toml::from_str(&text)
                .with_context(|| format!("decode {}", path.display())),
            Err(error) if error.kind() == ErrorKind::NotFound => Ok(Self::default()),
            Err(error) => Err(error).with_context(|| format!("read {}", path.display())),
        }
    }

    pub fn save_to(&self, path: &Path) -> Result<()> {
        let dir = path
            .parent()
            .with_context(|| format!("state path has no parent: {}", path.display()))?;
        fs::create_dir_all(dir).with_context(|| format!("create {}", dir.display()))?;

        let text = toml::to_string_pretty(self).context("encode state toml")?;
        let tmp = tmp_path(path);

        if let Err(error) = fs::write(&tmp, &text) {
            let _ = fs::remove_file(&tmp);
            return Err(error).with_context(|| format!("write {}", tmp.display()));
        }
        if let Err(error) = fs::rename(&tmp, path) {
            let _ = fs::remove_file(&tmp);
            return Err(error)
                .with_context(|| format!("rename {} -> {}", tmp.display(), path.display()));
        }
        Ok(())
    }
}

pub fn path() -> Result<PathBuf> {
    Ok(config_dir()?.join("state.toml"))
}

pub fn mark_auth_completed() -> Result<()> {
    let p = path()?;
    let mut state = AppState::load_from(&p)?;
    state.auth_completed = true;
    state.save_to(&p)
}

fn tmp_path(path: &Path) -> PathBuf {
    let mut name = path
        .file_name()
        .map(|n| n.to_os_string())
        .unwrap_or_default();
    name.push(format!(".tmp.{}", std::process::id()));
    path.with_file_name(name)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Mutex, atomic::{AtomicUsize, Ordering}};

    static NEXT_TEST_DIR: AtomicUsize = AtomicUsize::new(0);
    static ENV_LOCK: Mutex<()> = Mutex::new(());

    fn temp_dir() -> PathBuf {
        let unique = NEXT_TEST_DIR.fetch_add(1, Ordering::Relaxed);
        let dir = std::env::temp_dir().join(format!(
            "chatter-state-test-{}-{unique}",
            std::process::id()
        ));
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn load_returns_default_when_file_is_missing() {
        let dir = temp_dir();
        let path = dir.join("state.toml");
        let state = AppState::load_from(&path).unwrap();
        assert!(!state.auth_completed);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn save_and_load_round_trips_auth_completed() {
        let dir = temp_dir();
        let path = dir.join("state.toml");
        let state = AppState { auth_completed: true };
        state.save_to(&path).unwrap();
        let loaded = AppState::load_from(&path).unwrap();
        assert!(loaded.auth_completed);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn mark_auth_completed_writes_flag() {
        let _guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let dir = temp_dir();
        let prev = std::env::var_os("CHATTER_CONFIG_DIR");
        unsafe { std::env::set_var("CHATTER_CONFIG_DIR", &dir) };

        mark_auth_completed().unwrap();

        let state = AppState::load_from(&dir.join("state.toml")).unwrap();
        assert!(state.auth_completed);

        match prev {
            Some(v) => unsafe { std::env::set_var("CHATTER_CONFIG_DIR", v) },
            None => unsafe { std::env::remove_var("CHATTER_CONFIG_DIR") },
        }
        let _ = fs::remove_dir_all(&dir);
    }
}
