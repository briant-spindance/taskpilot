use anyhow::{bail, Context, Result};
use colored::Colorize;
use serde::Deserialize;
use std::fs;
use std::io::{self, BufRead, Write};
use std::path::PathBuf;

const CONFIG_DIR: &str = "taskpilot";
const CONFIG_FILE: &str = "config.yml";

#[derive(Debug, Deserialize, Default)]
pub struct Config {
    /// Anthropic API key (overridden by ANTHROPIC_API_KEY env var or project .env)
    pub api_key: Option<String>,
    /// Default model (overridden by --model flag or recipe model field)
    pub model: Option<String>,
    /// Default streaming behavior (overridden by --no-stream flag)
    pub stream: Option<bool>,
}

/// Resolve the config file path: ~/.local/taskpilot/config.yml
fn config_path() -> Option<PathBuf> {
    std::env::var_os("HOME").map(|home| {
        PathBuf::from(home)
            .join(".local")
            .join(CONFIG_DIR)
            .join(CONFIG_FILE)
    })
}

/// Load the global config file. Returns default config if the file doesn't exist.
pub fn load() -> Config {
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
pub fn path_display() -> String {
    config_path()
        .map(|p| p.display().to_string())
        .unwrap_or_else(|| "~/.local/taskpilot/config.yml".to_string())
}

/// Interactive setup: prompt for values and write config.yml.
pub fn setup() -> Result<()> {
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
        io::stdin().lock().read_line(&mut answer)?;
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
        io::stdin().lock().read_line(&mut answer)?;
        if answer.trim().eq_ignore_ascii_case("n") {
            None
        } else {
            Some(key.clone())
        }
    } else {
        print!("  Anthropic API key: ");
        io::stdout().flush()?;
        let mut key = String::new();
        io::stdin().lock().read_line(&mut key)?;
        let key = key.trim().to_string();
        if key.is_empty() { None } else { Some(key) }
    };

    // Model
    let default_model = "claude-sonnet-4-20250514";
    print!("  Default model [{}]: ", default_model.dimmed());
    io::stdout().flush()?;
    let mut model_input = String::new();
    io::stdin().lock().read_line(&mut model_input)?;
    let model = model_input.trim();
    let model = if model.is_empty() { default_model } else { model };

    // Streaming
    print!("  Enable streaming by default? [Y/n] ");
    io::stdout().flush()?;
    let mut stream_input = String::new();
    io::stdin().lock().read_line(&mut stream_input)?;
    let stream = !stream_input.trim().eq_ignore_ascii_case("n");

    // Write config
    let mut yaml = String::new();
    if let Some(ref key) = api_key {
        yaml.push_str(&format!("api_key: {key}\n"));
    } else {
        yaml.push_str("# api_key: sk-ant-...\n");
    }
    yaml.push_str(&format!("model: {model}\n"));
    yaml.push_str(&format!("stream: {stream}\n"));

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

fn mask_key(key: &str) -> String {
    if key.len() <= 8 {
        "****".to_string()
    } else {
        format!("{}...{}", &key[..4], &key[key.len() - 4..])
    }
}
