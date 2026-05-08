use std::{
    fmt,
    fs::{File, OpenOptions},
    io::Write,
    sync::{Mutex, OnceLock},
    time::{SystemTime, UNIX_EPOCH},
};

use anyhow::{Context, Result};

static LOG_FILE: OnceLock<Mutex<File>> = OnceLock::new();

pub fn init(path: &str) -> Result<()> {
    let file = OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
        .with_context(|| format!("open log file {path}"))?;

    let _ = LOG_FILE.set(Mutex::new(file));
    info(format_args!("logging initialized path={path}"));
    Ok(())
}

pub fn info(args: fmt::Arguments<'_>) {
    write("INFO", args);
}

pub fn warn(args: fmt::Arguments<'_>) {
    write("WARN", args);
}

pub fn error(args: fmt::Arguments<'_>) {
    write("ERROR", args);
}

fn write(level: &str, args: fmt::Arguments<'_>) {
    let Some(file) = LOG_FILE.get() else {
        return;
    };

    let timestamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .unwrap_or_default();

    if let Ok(mut file) = file.lock() {
        let _ = writeln!(file, "{timestamp} {level} {args}");
    }
}
