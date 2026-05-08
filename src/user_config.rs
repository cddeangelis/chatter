use std::{
    collections::HashMap,
    fs,
    io::ErrorKind,
    path::{Path, PathBuf},
};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

use crate::session::config_dir;

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct UserConfig {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    #[serde(flatten)]
    extra: HashMap<String, serde_json::Value>,
}

impl UserConfig {
    pub fn load() -> Result<Self> {
        Self::load_from(&path()?)
    }

    fn load_from(path: &Path) -> Result<Self> {
        match fs::read_to_string(path) {
            Ok(json) => serde_json::from_str(&json)
                .with_context(|| format!("decode {}", path.display())),
            Err(error) if error.kind() == ErrorKind::NotFound => Ok(Self::default()),
            Err(error) => Err(error).with_context(|| format!("read {}", path.display())),
        }
    }

    fn save_to(&self, path: &Path) -> Result<()> {
        let dir = path
            .parent()
            .with_context(|| format!("config path has no parent: {}", path.display()))?;
        fs::create_dir_all(dir).with_context(|| format!("create {}", dir.display()))?;

        let json = serde_json::to_string_pretty(self).context("encode user config json")?;
        let tmp = tmp_path(path);

        if let Err(error) = fs::write(&tmp, &json) {
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

pub fn set_model(model: &str) -> Result<()> {
    let path = path()?;
    let mut cfg = UserConfig::load_from(&path)?;
    cfg.model = Some(model.to_string());
    cfg.save_to(&path)
}

pub fn path() -> Result<PathBuf> {
    Ok(config_dir()?.join("config.json"))
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
    use std::sync::atomic::{AtomicUsize, Ordering};

    static NEXT_TEST_DIR: AtomicUsize = AtomicUsize::new(0);

    fn temp_dir() -> PathBuf {
        let unique = NEXT_TEST_DIR.fetch_add(1, Ordering::Relaxed);
        let dir = std::env::temp_dir().join(format!(
            "chatter-user-config-test-{}-{unique}",
            std::process::id()
        ));
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn load_returns_default_when_file_is_missing() {
        let dir = temp_dir();
        let path = dir.join("config.json");

        let cfg = UserConfig::load_from(&path).unwrap();

        assert!(cfg.model.is_none());
        assert!(cfg.extra.is_empty());

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn save_then_load_round_trips_model() {
        let dir = temp_dir();
        let path = dir.join("config.json");

        let cfg = UserConfig {
            model: Some("gpt-9000".to_string()),
            extra: HashMap::new(),
        };
        cfg.save_to(&path).unwrap();

        let loaded = UserConfig::load_from(&path).unwrap();
        assert_eq!(loaded.model.as_deref(), Some("gpt-9000"));

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn saving_preserves_unknown_keys() {
        let dir = temp_dir();
        let path = dir.join("config.json");
        fs::write(
            &path,
            r#"{"model":"old","future_setting":{"nested":42}}"#,
        )
        .unwrap();

        let mut cfg = UserConfig::load_from(&path).unwrap();
        cfg.model = Some("new".to_string());
        cfg.save_to(&path).unwrap();

        let raw = fs::read_to_string(&path).unwrap();
        let value: serde_json::Value = serde_json::from_str(&raw).unwrap();
        assert_eq!(value["model"], serde_json::Value::String("new".to_string()));
        assert_eq!(value["future_setting"]["nested"], serde_json::json!(42));

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn save_leaves_no_temp_files_behind() {
        let dir = temp_dir();
        let path = dir.join("config.json");

        UserConfig {
            model: Some("m".to_string()),
            extra: HashMap::new(),
        }
        .save_to(&path)
        .unwrap();

        let leftovers: Vec<_> = fs::read_dir(&dir)
            .unwrap()
            .map(|entry| entry.unwrap().file_name())
            .filter(|name| {
                name.to_string_lossy().contains(".tmp.")
            })
            .collect();
        assert!(leftovers.is_empty(), "found temp leftovers: {leftovers:?}");

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn omits_model_key_when_none() {
        let cfg = UserConfig::default();
        let json = serde_json::to_string(&cfg).unwrap();
        assert!(!json.contains("model"), "expected no model key, got {json}");
    }
}
