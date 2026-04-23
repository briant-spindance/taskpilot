use anyhow::{bail, Context, Result};
use colored::Colorize;
use serde::Deserialize;
use std::fs;
use std::io::{self, BufRead, Write};
use std::path::PathBuf;

const CONFIG_DIR: &str = "taskpilot";
const CONFIG_FILE: &str = "config.yml";

#[derive(Debug, Deserialize, Default)]
pub(crate) struct Config {
    /// Anthropic API key (overridden by ANTHROPIC_API_KEY env var or project .env)
    pub(crate) api_key: Option<String>,
    /// Default model (overridden by --model flag or recipe model field)
    pub(crate) model: Option<String>,
    /// Default streaming behavior (overridden by --no-stream flag)
    pub(crate) stream: Option<bool>,
    /// Whether bash tool is allowed by default (overridden by --allow-bash flag or recipe field)
    pub(crate) allow_bash: Option<bool>,
}

/// Resolve the config file path: ~/.local/taskpilot/config.yml
fn config_path() -> Option<PathBuf> {
    crate::constants::home_dir().ok().map(|home| {
        home.join(".local")
            .join(CONFIG_DIR)
            .join(CONFIG_FILE)
    })
}

/// Load the global config file. Returns default config if the file doesn't exist.
pub(crate) fn load() -> Config {
    let path = match config_path() {
        Some(p) => p,
        None => return Config::default(),
    };

    if !path.exists() {
        return Config::default();
    }

    let content = match fs::read_to_string(&path) {
        Ok(c) => c,
        Err(_) => return Config::default(),
    };

    match serde_yaml::from_str(&content) {
        Ok(cfg) => cfg,
        Err(e) => {
            eprintln!(
                "warning: failed to parse {}: {e}",
                path.display()
            );
            Config::default()
        }
    }
}

/// Get the config file path (for display in doctor/help).
pub(crate) fn path_display() -> String {
    config_path()
        .map(|p| p.display().to_string())
        .unwrap_or_else(|| "~/.local/taskpilot/config.yml".to_string())
}

/// Interactive setup: prompt for values and write config.yml.
pub(crate) fn setup() -> Result<()> {
    setup_with_reader(&mut io::stdin().lock())
}

/// Testable setup that reads interactive input from the provided reader.
pub(crate) fn setup_with_reader(reader: &mut dyn BufRead) -> Result<()> {
    let path = config_path()
        .context("could not resolve home directory")?;

    if path.exists() {
        eprintln!(
            "{} Config already exists at {}",
            "!".yellow().bold(),
            path.display().to_string().dimmed()
        );
        eprint!("  Overwrite? [y/N] ");
        io::stderr().flush()?;
        let mut answer = String::new();
        reader.read_line(&mut answer)?;
        if !answer.trim().eq_ignore_ascii_case("y") {
            bail!("cancelled");
        }
        eprintln!();
    }

    println!("{}", "taskpilot config".bold());
    println!();

    // API key
    let existing_key = std::env::var("ANTHROPIC_API_KEY").ok();
    let api_key = if let Some(ref key) = existing_key {
        let masked = mask_key(key);
        println!("  ANTHROPIC_API_KEY is already set in your environment: {}", masked.dimmed());
        print!("  Store it in config? [Y/n] ");
        io::stdout().flush()?;
        let mut answer = String::new();
        reader.read_line(&mut answer)?;
        if answer.trim().eq_ignore_ascii_case("n") {
            None
        } else {
            Some(key.clone())
        }
    } else {
        print!("  Anthropic API key: ");
        io::stdout().flush()?;
        let mut key = String::new();
        reader.read_line(&mut key)?;
        let key = key.trim().to_string();
        if key.is_empty() { None } else { Some(key) }
    };

    // Model
    let default_model = crate::constants::DEFAULT_MODEL;
    print!("  Default model [{}]: ", default_model.dimmed());
    io::stdout().flush()?;
    let mut model_input = String::new();
    reader.read_line(&mut model_input)?;
    let model = model_input.trim();
    let model = if model.is_empty() { default_model } else { model };

    // Streaming
    print!("  Enable streaming by default? [Y/n] ");
    io::stdout().flush()?;
    let mut stream_input = String::new();
    reader.read_line(&mut stream_input)?;
    let stream = !stream_input.trim().eq_ignore_ascii_case("n");

    // Bash
    print!("  Allow bash tool by default? [y/N] ");
    io::stdout().flush()?;
    let mut bash_input = String::new();
    reader.read_line(&mut bash_input)?;
    let allow_bash = bash_input.trim().eq_ignore_ascii_case("y");

    // Write config
    let mut yaml = String::new();
    if let Some(ref key) = api_key {
        yaml.push_str(&format!("api_key: {key}\n"));
    } else {
        yaml.push_str("# api_key: sk-ant-...\n");
    }
    yaml.push_str(&format!("model: {model}\n"));
    yaml.push_str(&format!("stream: {stream}\n"));
    yaml.push_str(&format!("allow_bash: {allow_bash}\n"));

    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .with_context(|| format!("create directory {}", parent.display()))?;
    }
    fs::write(&path, &yaml)
        .with_context(|| format!("write {}", path.display()))?;

    println!();
    println!("{} Wrote {}", "✓".green(), path.display());
    println!("  Run {} to verify your setup.", "taskpilot doctor".bold());
    Ok(())
}

pub(crate) fn mask_key(key: &str) -> String {
    if key.len() <= 8 {
        "****".to_string()
    } else {
        format!("{}...{}", &key[..4], &key[key.len() - 4..])
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serial_test::serial;
    use std::io::Cursor;
    use tempfile::TempDir;

    /// Set HOME to a temp dir and return the TempDir (must stay alive).
    fn set_home(tmp: &TempDir) {
        std::env::set_var("HOME", tmp.path());
    }

    fn config_file_path(tmp: &TempDir) -> PathBuf {
        tmp.path()
            .join(".local")
            .join(CONFIG_DIR)
            .join(CONFIG_FILE)
    }

    // ── mask_key ──────────────────────────────────────────────

    #[test]
    fn mask_key_short() {
        assert_eq!(mask_key("abc"), "****");
        assert_eq!(mask_key("12345678"), "****");
    }

    #[test]
    fn mask_key_long() {
        assert_eq!(mask_key("abcdefghi"), "abcd...fghi");
        assert_eq!(mask_key("sk-ant-api03-XXXXXXXXXXXX-YYYY"), "sk-a...YYYY");
    }

    // ── load ──────────────────────────────────────────────────

    #[test]
    #[serial]
    fn load_no_config_file() {
        let tmp = TempDir::new().unwrap();
        set_home(&tmp);
        let cfg = load();
        assert!(cfg.api_key.is_none());
        assert!(cfg.model.is_none());
        assert!(cfg.stream.is_none());
        assert!(cfg.allow_bash.is_none());
    }

    #[test]
    #[serial]
    fn load_invalid_yaml() {
        let tmp = TempDir::new().unwrap();
        set_home(&tmp);
        let path = config_file_path(&tmp);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(&path, "{{{{not yaml at all::::").unwrap();
        let cfg = load();
        assert!(cfg.api_key.is_none());
    }

    #[test]
    #[serial]
    fn load_valid_yaml() {
        let tmp = TempDir::new().unwrap();
        set_home(&tmp);
        let path = config_file_path(&tmp);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(
            &path,
            "api_key: sk-test\nmodel: claude-3\nstream: false\nallow_bash: true\n",
        )
        .unwrap();
        let cfg = load();
        assert_eq!(cfg.api_key.as_deref(), Some("sk-test"));
        assert_eq!(cfg.model.as_deref(), Some("claude-3"));
        assert_eq!(cfg.stream, Some(false));
        assert_eq!(cfg.allow_bash, Some(true));
    }

    #[test]
    #[serial]
    fn load_partial_yaml() {
        let tmp = TempDir::new().unwrap();
        set_home(&tmp);
        let path = config_file_path(&tmp);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(&path, "model: gpt-4\n").unwrap();
        let cfg = load();
        assert!(cfg.api_key.is_none());
        assert_eq!(cfg.model.as_deref(), Some("gpt-4"));
        assert!(cfg.stream.is_none());
        assert!(cfg.allow_bash.is_none());
    }

    // ── path_display ──────────────────────────────────────────

    #[test]
    #[serial]
    fn path_display_with_home() {
        let tmp = TempDir::new().unwrap();
        set_home(&tmp);
        let display = path_display();
        assert!(display.contains(".local/taskpilot/config.yml"));
        assert!(display.starts_with(tmp.path().to_str().unwrap()));
    }

    #[test]
    #[serial]
    fn path_display_without_home() {
        std::env::remove_var("HOME");
        let display = path_display();
        assert_eq!(display, "~/.local/taskpilot/config.yml");
        // Restore HOME so other tests don't break
        std::env::set_var("HOME", "/tmp");
    }

    // ── setup_with_reader ─────────────────────────────────────

    /// Helper: build a Cursor from newline-separated input lines.
    fn input(lines: &[&str]) -> Cursor<Vec<u8>> {
        let data = lines.join("\n") + "\n";
        Cursor::new(data.into_bytes())
    }

    #[test]
    #[serial]
    fn setup_fresh_all_defaults() {
        let tmp = TempDir::new().unwrap();
        set_home(&tmp);
        std::env::remove_var("ANTHROPIC_API_KEY");

        // api_key, model (empty=default), stream (empty=Y), bash (empty=N)
        let mut reader = input(&["sk-ant-test-key", "", "", ""]);
        setup_with_reader(&mut reader).unwrap();

        let content = fs::read_to_string(config_file_path(&tmp)).unwrap();
        assert!(content.contains("api_key: sk-ant-test-key"));
        assert!(content.contains("model: claude-sonnet-4-20250514"));
        assert!(content.contains("stream: true"));
        assert!(content.contains("allow_bash: false"));
    }

    #[test]
    #[serial]
    fn setup_fresh_custom_values() {
        let tmp = TempDir::new().unwrap();
        set_home(&tmp);
        std::env::remove_var("ANTHROPIC_API_KEY");

        // api_key, model custom, stream n, bash y
        let mut reader = input(&["my-key", "gpt-4o", "n", "y"]);
        setup_with_reader(&mut reader).unwrap();

        let content = fs::read_to_string(config_file_path(&tmp)).unwrap();
        assert!(content.contains("api_key: my-key"));
        assert!(content.contains("model: gpt-4o"));
        assert!(content.contains("stream: false"));
        assert!(content.contains("allow_bash: true"));
    }

    #[test]
    #[serial]
    fn setup_fresh_empty_api_key() {
        let tmp = TempDir::new().unwrap();
        set_home(&tmp);
        std::env::remove_var("ANTHROPIC_API_KEY");

        let mut reader = input(&["", "", "", ""]);
        setup_with_reader(&mut reader).unwrap();

        let content = fs::read_to_string(config_file_path(&tmp)).unwrap();
        assert!(content.contains("# api_key: sk-ant-..."));
    }

    #[test]
    #[serial]
    fn setup_existing_overwrite_yes() {
        let tmp = TempDir::new().unwrap();
        set_home(&tmp);
        std::env::remove_var("ANTHROPIC_API_KEY");

        let path = config_file_path(&tmp);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(&path, "model: old\n").unwrap();

        // overwrite=y, api_key, model, stream, bash
        let mut reader = input(&["y", "new-key", "new-model", "", ""]);
        setup_with_reader(&mut reader).unwrap();

        let content = fs::read_to_string(&path).unwrap();
        assert!(content.contains("api_key: new-key"));
        assert!(content.contains("model: new-model"));
    }

    #[test]
    #[serial]
    fn setup_existing_overwrite_declined() {
        let tmp = TempDir::new().unwrap();
        set_home(&tmp);
        std::env::remove_var("ANTHROPIC_API_KEY");

        let path = config_file_path(&tmp);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(&path, "model: old\n").unwrap();

        let mut reader = input(&["n"]);
        let result = setup_with_reader(&mut reader);
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("cancelled"));

        // Original file untouched
        let content = fs::read_to_string(&path).unwrap();
        assert_eq!(content, "model: old\n");
    }

    #[test]
    #[serial]
    fn setup_api_key_from_env_store_yes() {
        let tmp = TempDir::new().unwrap();
        set_home(&tmp);
        std::env::set_var("ANTHROPIC_API_KEY", "env-key-123456789");

        // store=Y (default), model, stream, bash
        let mut reader = input(&["", "", "", ""]);
        setup_with_reader(&mut reader).unwrap();

        let content = fs::read_to_string(config_file_path(&tmp)).unwrap();
        assert!(content.contains("api_key: env-key-123456789"));

        std::env::remove_var("ANTHROPIC_API_KEY");
    }

    #[test]
    #[serial]
    fn setup_api_key_from_env_store_declined() {
        let tmp = TempDir::new().unwrap();
        set_home(&tmp);
        std::env::set_var("ANTHROPIC_API_KEY", "env-key-123456789");

        // store=n, model, stream, bash
        let mut reader = input(&["n", "", "", ""]);
        setup_with_reader(&mut reader).unwrap();

        let content = fs::read_to_string(config_file_path(&tmp)).unwrap();
        assert!(content.contains("# api_key: sk-ant-..."));

        std::env::remove_var("ANTHROPIC_API_KEY");
    }

    #[test]
    #[serial]
    fn setup_stream_no() {
        let tmp = TempDir::new().unwrap();
        set_home(&tmp);
        std::env::remove_var("ANTHROPIC_API_KEY");

        let mut reader = input(&["", "", "n", ""]);
        setup_with_reader(&mut reader).unwrap();

        let content = fs::read_to_string(config_file_path(&tmp)).unwrap();
        assert!(content.contains("stream: false"));
    }

    #[test]
    #[serial]
    fn setup_bash_yes() {
        let tmp = TempDir::new().unwrap();
        set_home(&tmp);
        std::env::remove_var("ANTHROPIC_API_KEY");

        let mut reader = input(&["", "", "", "y"]);
        setup_with_reader(&mut reader).unwrap();

        let content = fs::read_to_string(config_file_path(&tmp)).unwrap();
        assert!(content.contains("allow_bash: true"));
    }
}
