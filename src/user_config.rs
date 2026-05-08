use std::{
    collections::BTreeMap,
    fs,
    io::ErrorKind,
    path::{Path, PathBuf},
};

use anyhow::{Context, Result, anyhow};
use serde::{Deserialize, Serialize};

use crate::provider::Provider;

#[derive(Default, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct UserConfig {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub active: Option<String>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub profile: BTreeMap<String, Profile>,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Profile {
    pub provider: Provider,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub api_key: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub origin: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub system_prompt: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_tokens: Option<u32>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub custom_headers: BTreeMap<String, String>,
}

impl UserConfig {
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
            .with_context(|| format!("config path has no parent: {}", path.display()))?;
        fs::create_dir_all(dir).with_context(|| format!("create {}", dir.display()))?;

        let text = toml::to_string_pretty(self).context("encode user config toml")?;
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

    /// Sets `model` on the active profile. Returns `true` if a profile was updated.
    pub fn set_active_model(&mut self, model: &str) -> bool {
        let name = match &self.active {
            Some(n) => n.clone(),
            None => return false,
        };
        match self.profile.get_mut(&name) {
            Some(profile) => {
                profile.model = Some(model.to_string());
                true
            }
            None => false,
        }
    }
}

/// Loads config, updates the active profile's model, and saves. If there is no
/// active profile the selection is applied in-memory only for this session.
pub fn persist_active_model(model: &str) -> Result<()> {
    let path = path()?;
    let mut cfg = UserConfig::load_from(&path)?;
    if !cfg.set_active_model(model) {
        crate::logger::warn(format_args!(
            "model picker: no active profile in config.toml; model selection won't persist"
        ));
    } else {
        cfg.save_to(&path)?;
    }
    Ok(())
}

pub fn path() -> Result<PathBuf> {
    Ok(config_dir()?.join("config.toml"))
}

pub(crate) fn config_dir() -> Result<PathBuf> {
    if let Some(value) = std::env::var_os("CHATTER_CONFIG_DIR") {
        if !value.is_empty() {
            return Ok(PathBuf::from(value));
        }
    }
    Ok(home_dir()?.join(".chatter"))
}

pub(crate) fn home_dir() -> Result<PathBuf> {
    std::env::var_os("HOME")
        .map(PathBuf::from)
        .filter(|path| !path.as_os_str().is_empty())
        .filter(|path| Path::new(path).is_absolute())
        .ok_or_else(|| anyhow!("HOME is not set to an absolute path"))
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
    use std::sync::Mutex;
    use std::sync::atomic::{AtomicUsize, Ordering};

    static NEXT_TEST_DIR: AtomicUsize = AtomicUsize::new(0);
    static ENV_LOCK: Mutex<()> = Mutex::new(());

    fn temp_dir() -> PathBuf {
        let unique = NEXT_TEST_DIR.fetch_add(1, Ordering::Relaxed);
        let dir = std::env::temp_dir().join(format!(
            "chatter-user-config-test-{}-{unique}",
            std::process::id()
        ));
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn minimal_profile() -> Profile {
        Profile {
            provider: Provider::Anthropic,
            api_key: None,
            origin: None,
            model: None,
            system_prompt: None,
            max_tokens: None,
            custom_headers: BTreeMap::new(),
        }
    }

    fn with_profile(name: &str, profile: Profile) -> UserConfig {
        let mut cfg = UserConfig {
            active: Some(name.to_string()),
            profile: BTreeMap::new(),
        };
        cfg.profile.insert(name.to_string(), profile);
        cfg
    }

    #[test]
    fn load_returns_default_when_file_is_missing() {
        let dir = temp_dir();
        let path = dir.join("config.toml");

        let cfg = UserConfig::load_from(&path).unwrap();

        assert!(cfg.active.is_none());
        assert!(cfg.profile.is_empty());

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn save_then_load_round_trips_profile_model() {
        let dir = temp_dir();
        let path = dir.join("config.toml");

        let mut profile = minimal_profile();
        profile.model = Some("gpt-9000".to_string());
        let cfg = with_profile("main", profile);
        cfg.save_to(&path).unwrap();

        let loaded = UserConfig::load_from(&path).unwrap();
        assert_eq!(loaded.active.as_deref(), Some("main"));
        assert_eq!(loaded.profile["main"].model.as_deref(), Some("gpt-9000"));

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn deny_unknown_fields_rejects_typos() {
        let dir = temp_dir();
        let path = dir.join("config.toml");
        fs::write(&path, "active = \"x\"\nfuture_setting = 42\n").unwrap();

        let err = UserConfig::load_from(&path).unwrap_err();
        let msg = format!("{err:#}");
        assert!(
            msg.contains("future_setting") || msg.contains("unknown"),
            "expected unknown-field error, got: {msg}"
        );

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn save_leaves_no_temp_files_behind() {
        let dir = temp_dir();
        let path = dir.join("config.toml");

        with_profile("main", minimal_profile())
            .save_to(&path)
            .unwrap();

        let leftovers: Vec<_> = fs::read_dir(&dir)
            .unwrap()
            .map(|e| e.unwrap().file_name())
            .filter(|n| n.to_string_lossy().contains(".tmp."))
            .collect();
        assert!(leftovers.is_empty(), "found temp leftovers: {leftovers:?}");

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn set_active_model_writes_to_active_profile() {
        let dir = temp_dir();
        let path = dir.join("config.toml");

        with_profile("main", minimal_profile())
            .save_to(&path)
            .unwrap();

        let mut loaded = UserConfig::load_from(&path).unwrap();
        assert!(loaded.set_active_model("new-model"));
        loaded.save_to(&path).unwrap();

        let reloaded = UserConfig::load_from(&path).unwrap();
        assert_eq!(reloaded.profile["main"].model.as_deref(), Some("new-model"));

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn set_active_model_returns_false_when_no_active() {
        let mut cfg = UserConfig::default();
        assert!(!cfg.set_active_model("new-model"));
    }

    // config_dir tests (moved from session.rs)

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
}
