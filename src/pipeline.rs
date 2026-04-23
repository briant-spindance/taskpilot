use anyhow::{Context, Result};
use colored::Colorize;
use std::fs;

use crate::constants::DEFAULT_MODEL;
use crate::{config, recipe, runner, skill, workspace};

/// Resolved run configuration, produced by merging CLI flags, recipe fields,
/// and global config. Eliminates the duplicated resolution logic that
/// previously lived in both `dispatch_command` and `run_recipe`.
struct ResolvedRun {
    prompt: String,
    inputs: Vec<String>,
    output_dir: Option<String>,
    model: String,
    stream: bool,
    allow_bash: bool,
    skills: Vec<skill::Skill>,
}

/// Resolve prompt from the available sources (CLI flags > recipe fields).
fn resolve_prompt(
    cli_prompt: Option<&str>,
    cli_prompt_file: Option<&str>,
    recipe: Option<&recipe::Recipe>,
    recipe_name: Option<&str>,
) -> Result<String> {
    if let Some(pf) = cli_prompt_file {
        return fs::read_to_string(pf)
            .with_context(|| format!("read prompt file: {pf}"));
    }
    if let Some(p) = cli_prompt {
        return Ok(p.to_string());
    }
    if let Some(r) = recipe {
        if let Some(ref pf) = r.prompt_file {
            return fs::read_to_string(pf)
                .with_context(|| format!("read prompt file: {pf}"));
        }
        if let Some(ref p) = r.prompt {
            return Ok(p.clone());
        }
    }
    let context = recipe_name
        .map(|n| format!("recipe {n:?} has no prompt or prompt_file"))
        .unwrap_or_else(|| "--prompt or --prompt-file is required (or use a recipe name)".into());
    anyhow::bail!("{context}")
}

/// Resolve model from CLI > recipe > config > default.
fn resolve_model(
    cli_model: Option<&str>,
    recipe: Option<&recipe::Recipe>,
    global_config: &config::Config,
) -> String {
    cli_model
        .map(|s| s.to_string())
        .or_else(|| recipe.and_then(|r| r.model.clone()))
        .or_else(|| global_config.model.clone())
        .unwrap_or_else(|| DEFAULT_MODEL.to_string())
}

/// Resolve streaming: --no-stream flag > config > default (true).
fn resolve_stream(no_stream: bool, global_config: &config::Config) -> bool {
    if no_stream {
        false
    } else {
        global_config.stream.unwrap_or(true)
    }
}

/// Resolve allow_bash: CLI flag > recipe > config > default (false).
fn resolve_allow_bash(
    cli_allow_bash: bool,
    recipe: Option<&recipe::Recipe>,
    global_config: &config::Config,
) -> bool {
    if cli_allow_bash {
        true
    } else {
        recipe
            .and_then(|r| r.allow_bash)
            .or(global_config.allow_bash)
            .unwrap_or(false)
    }
}

/// Build a fully resolved run configuration from the various sources.
fn resolve_run(
    cli_prompt: Option<&str>,
    cli_prompt_file: Option<&str>,
    cli_input: &[String],
    cli_output_dir: Option<&str>,
    cli_model: Option<&str>,
    no_stream: bool,
    cli_allow_bash: bool,
    recipe: Option<&recipe::Recipe>,
    recipe_name: Option<&str>,
    extra_skills_dirs: &[String],
    global_config: &config::Config,
) -> Result<ResolvedRun> {
    let prompt = resolve_prompt(cli_prompt, cli_prompt_file, recipe, recipe_name)?;

    let inputs = if !cli_input.is_empty() {
        cli_input.to_vec()
    } else {
        recipe.map(|r| r.input.clone()).unwrap_or_default()
    };

    let output_dir = cli_output_dir
        .map(|s| s.to_string())
        .or_else(|| recipe.and_then(|r| r.output_dir.clone()));

    let model = resolve_model(cli_model, recipe, global_config);
    let stream = resolve_stream(no_stream, global_config);
    let allow_bash = resolve_allow_bash(cli_allow_bash, recipe, global_config);
    let skills = skill::discover(extra_skills_dirs).context("discover skills")?;

    Ok(ResolvedRun {
        prompt,
        inputs,
        output_dir,
        model,
        stream,
        allow_bash,
        skills,
    })
}

/// Execute a resolved run: create workspace, stage inputs, run agent, collect outputs.
fn execute_run(resolved: &ResolvedRun, api_client: &dyn runner::ApiClient) -> Result<()> {
    let ws = workspace::Workspace::new()?;

    if !resolved.inputs.is_empty() {
        ws.stage_inputs(&resolved.inputs)?;
    }

    runner::run_with_client(
        &runner::Config {
            model: resolved.model.clone(),
            prompt: resolved.prompt.clone(),
            skills: resolved.skills.clone(),
            work_dir: ws.dir.to_string_lossy().to_string(),
            stream: resolved.stream,
            allow_bash: resolved.allow_bash,
        },
        api_client,
    )?;

    if let Some(ref out) = resolved.output_dir {
        ws.collect_outputs(out)?;
    }

    Ok(())
}

/// Run a recipe by name, used for dependency execution and the main run path.
pub(crate) fn run_recipe(
    name: &str,
    cli_input: &[String],
    cli_output_dir: Option<&str>,
    cli_model: Option<&str>,
    cli_prompt: Option<&str>,
    no_stream: bool,
    extra_skills_dirs: &[String],
    global_config: &config::Config,
    api_client: &dyn runner::ApiClient,
) -> Result<()> {
    let r = recipe::get(name)?;

    // Resolve skill dependencies
    if !r.skill_deps.is_empty() {
        recipe::resolve_skill_deps(&r.skill_deps, extra_skills_dirs)?;
    }

    let resolved = resolve_run(
        cli_prompt,
        None, // cli_prompt_file not supported for dependency runs
        cli_input,
        cli_output_dir,
        cli_model,
        no_stream,
        false, // cli_allow_bash not forwarded to dependency runs
        Some(&r),
        Some(name),
        extra_skills_dirs,
        global_config,
    )?;

    execute_run(&resolved, api_client)
}

/// Full run command handler — handles both recipe-based and ad-hoc runs.
pub(crate) fn run_command(
    recipe_name: Option<String>,
    prompt: Option<String>,
    prompt_file: Option<String>,
    input: Vec<String>,
    output_dir: Option<String>,
    model: Option<String>,
    dry_run: bool,
    no_stream: bool,
    skills_dir: Vec<String>,
    cli_allow_bash: bool,
    global_config: &config::Config,
    api_client: &dyn runner::ApiClient,
) -> Result<()> {
    let recipe = if let Some(ref name) = recipe_name {
        // Resolve depends_on chain first
        let execution_order = recipe::resolve_depends_on(name)?;

        // Run all dependencies (everything except the last, which is the target)
        if execution_order.len() > 1 {
            let deps = &execution_order[..execution_order.len() - 1];
            eprintln!(
                "{}",
                format!("Running {} dependencies first...", deps.len()).dimmed()
            );
            for dep_name in deps {
                eprintln!("\n{} {}", "▶ dependency:".cyan().bold(), dep_name.bold());
                run_recipe(
                    dep_name, &[], None, None, None, no_stream, &skills_dir, global_config,
                    api_client,
                )?;
            }
            eprintln!("\n{} {}", "▶ target:".green().bold(), name.bold());
        }

        let r = recipe::get(name)?;

        // Resolve skill dependencies before running
        if !r.skill_deps.is_empty() {
            eprintln!("{}", "Checking skill dependencies...".dimmed());
            recipe::resolve_skill_deps(&r.skill_deps, &skills_dir)?;
            eprintln!();
        }

        Some(r)
    } else {
        None
    };

    let resolved = resolve_run(
        prompt.as_deref(),
        prompt_file.as_deref(),
        &input,
        output_dir.as_deref(),
        model.as_deref(),
        no_stream,
        cli_allow_bash,
        recipe.as_ref(),
        recipe_name.as_deref(),
        &skills_dir,
        global_config,
    )?;

    if dry_run {
        println!("=== Dry Run ===");
        if let Some(ref name) = recipe_name {
            println!("Recipe: {name}");
        }
        println!("Model: {}", resolved.model);
        println!("Prompt: {}", resolved.prompt);
        println!("Skills: {} discovered", resolved.skills.len());
        for s in &resolved.skills {
            println!("  - {} ({})", s.name, s.path.display());
        }
        println!("Inputs: {:?}", resolved.inputs);
        println!(
            "Output dir: {}",
            resolved.output_dir.as_deref().unwrap_or("(none)")
        );
        println!(
            "Bash: {}",
            if resolved.allow_bash {
                "enabled"
            } else {
                "disabled"
            }
        );
        return Ok(());
    }

    execute_run(&resolved, api_client)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serial_test::serial;

    // ── resolve helpers ──

    #[test]
    fn resolve_prompt_from_cli() {
        let p = resolve_prompt(Some("hello"), None, None, None).unwrap();
        assert_eq!(p, "hello");
    }

    #[test]
    fn resolve_prompt_from_cli_file() {
        let tmp = tempfile::TempDir::new().unwrap();
        let pf = tmp.path().join("p.txt");
        std::fs::write(&pf, "from file").unwrap();
        let p = resolve_prompt(None, Some(pf.to_str().unwrap()), None, None).unwrap();
        assert_eq!(p, "from file");
    }

    #[test]
    fn resolve_prompt_cli_file_overrides_cli_prompt() {
        let tmp = tempfile::TempDir::new().unwrap();
        let pf = tmp.path().join("p.txt");
        std::fs::write(&pf, "file wins").unwrap();
        let p = resolve_prompt(Some("ignored"), Some(pf.to_str().unwrap()), None, None).unwrap();
        assert_eq!(p, "file wins");
    }

    #[test]
    fn resolve_prompt_from_recipe_prompt() {
        let r = recipe::Recipe {
            description: None,
            prompt: Some("recipe prompt".into()),
            prompt_file: None,
            input: vec![],
            output_dir: None,
            model: None,
            skill_deps: vec![],
            depends_on: vec![],
            allow_bash: None,
        };
        let p = resolve_prompt(None, None, Some(&r), Some("test")).unwrap();
        assert_eq!(p, "recipe prompt");
    }

    #[test]
    fn resolve_prompt_from_recipe_file() {
        let tmp = tempfile::TempDir::new().unwrap();
        let pf = tmp.path().join("rp.txt");
        std::fs::write(&pf, "recipe file").unwrap();
        let r = recipe::Recipe {
            description: None,
            prompt: None,
            prompt_file: Some(pf.to_str().unwrap().to_string()),
            input: vec![],
            output_dir: None,
            model: None,
            skill_deps: vec![],
            depends_on: vec![],
            allow_bash: None,
        };
        let p = resolve_prompt(None, None, Some(&r), Some("test")).unwrap();
        assert_eq!(p, "recipe file");
    }

    #[test]
    fn resolve_prompt_none_errors_with_recipe_name() {
        let r = recipe::Recipe {
            description: None,
            prompt: None,
            prompt_file: None,
            input: vec![],
            output_dir: None,
            model: None,
            skill_deps: vec![],
            depends_on: vec![],
            allow_bash: None,
        };
        let err = resolve_prompt(None, None, Some(&r), Some("test")).unwrap_err();
        assert!(err.to_string().contains("recipe \"test\""));
    }

    #[test]
    fn resolve_prompt_none_errors_no_recipe() {
        let err = resolve_prompt(None, None, None, None).unwrap_err();
        assert!(err.to_string().contains("--prompt"));
    }

    #[test]
    fn resolve_model_cli_wins() {
        let cfg = config::Config {
            model: Some("config-model".into()),
            ..Default::default()
        };
        let r = recipe::Recipe {
            description: None,
            prompt: None,
            prompt_file: None,
            input: vec![],
            output_dir: None,
            model: Some("recipe-model".into()),
            skill_deps: vec![],
            depends_on: vec![],
            allow_bash: None,
        };
        assert_eq!(resolve_model(Some("cli-model"), Some(&r), &cfg), "cli-model");
    }

    #[test]
    fn resolve_model_recipe_over_config() {
        let cfg = config::Config {
            model: Some("config-model".into()),
            ..Default::default()
        };
        let r = recipe::Recipe {
            description: None,
            prompt: None,
            prompt_file: None,
            input: vec![],
            output_dir: None,
            model: Some("recipe-model".into()),
            skill_deps: vec![],
            depends_on: vec![],
            allow_bash: None,
        };
        assert_eq!(resolve_model(None, Some(&r), &cfg), "recipe-model");
    }

    #[test]
    fn resolve_model_config_over_default() {
        let cfg = config::Config {
            model: Some("config-model".into()),
            ..Default::default()
        };
        assert_eq!(resolve_model(None, None, &cfg), "config-model");
    }

    #[test]
    fn resolve_model_default() {
        let cfg = config::Config::default();
        assert_eq!(resolve_model(None, None, &cfg), DEFAULT_MODEL);
    }

    #[test]
    fn resolve_stream_no_stream_flag() {
        let cfg = config::Config {
            stream: Some(true),
            ..Default::default()
        };
        assert!(!resolve_stream(true, &cfg));
    }

    #[test]
    fn resolve_stream_from_config() {
        let cfg = config::Config {
            stream: Some(false),
            ..Default::default()
        };
        assert!(!resolve_stream(false, &cfg));
    }

    #[test]
    fn resolve_stream_default() {
        let cfg = config::Config::default();
        assert!(resolve_stream(false, &cfg));
    }

    #[test]
    fn resolve_allow_bash_cli_wins() {
        let cfg = config::Config::default();
        assert!(resolve_allow_bash(true, None, &cfg));
    }

    #[test]
    fn resolve_allow_bash_recipe_over_config() {
        let cfg = config::Config {
            allow_bash: Some(false),
            ..Default::default()
        };
        let r = recipe::Recipe {
            description: None,
            prompt: None,
            prompt_file: None,
            input: vec![],
            output_dir: None,
            model: None,
            skill_deps: vec![],
            depends_on: vec![],
            allow_bash: Some(true),
        };
        assert!(resolve_allow_bash(false, Some(&r), &cfg));
    }

    #[test]
    fn resolve_allow_bash_default() {
        let cfg = config::Config::default();
        assert!(!resolve_allow_bash(false, None, &cfg));
    }

    // ── run_recipe tests (moved from main.rs) ──

    use crate::testutil::MockApiClient;
    use std::cell::RefCell;
    use std::collections::VecDeque;

    #[test]
    #[serial]
    fn run_recipe_with_prompt() {
        let tmp = tempfile::TempDir::new().unwrap();
        let canonical = tmp.path().canonicalize().unwrap();
        std::env::set_current_dir(&canonical).unwrap();
        std::env::set_var("HOME", canonical.to_str().unwrap());
        std::env::set_var("ANTHROPIC_API_KEY", "test-key");

        std::fs::write(
            canonical.join("taskpilot.toml"),
            r#"
[recipes.test]
prompt = "do the thing"
output_dir = "out/"
"#,
        )
        .unwrap();

        let client = MockApiClient::immediate_end();
        let cfg = config::Config::default();
        let result = run_recipe("test", &[], None, None, None, true, &[], &cfg, &client);
        assert!(result.is_ok());
        assert!(canonical.join("out").exists());

        std::env::remove_var("ANTHROPIC_API_KEY");
    }

    #[test]
    #[serial]
    fn run_recipe_with_cli_prompt_override() {
        let tmp = tempfile::TempDir::new().unwrap();
        let canonical = tmp.path().canonicalize().unwrap();
        std::env::set_current_dir(&canonical).unwrap();
        std::env::set_var("HOME", canonical.to_str().unwrap());
        std::env::set_var("ANTHROPIC_API_KEY", "test-key");

        std::fs::write(
            canonical.join("taskpilot.toml"),
            r#"
[recipes.test]
prompt = "original"
"#,
        )
        .unwrap();

        let client = MockApiClient::immediate_end();
        let cfg = config::Config::default();
        let result = run_recipe(
            "test", &[], None, None, Some("override prompt"), true, &[], &cfg, &client,
        );
        assert!(result.is_ok());

        std::env::remove_var("ANTHROPIC_API_KEY");
    }

    #[test]
    #[serial]
    fn run_recipe_with_prompt_file() {
        let tmp = tempfile::TempDir::new().unwrap();
        let canonical = tmp.path().canonicalize().unwrap();
        std::env::set_current_dir(&canonical).unwrap();
        std::env::set_var("HOME", canonical.to_str().unwrap());
        std::env::set_var("ANTHROPIC_API_KEY", "test-key");

        std::fs::write(canonical.join("my-prompt.md"), "file prompt").unwrap();
        std::fs::write(
            canonical.join("taskpilot.toml"),
            r#"
[recipes.test]
prompt_file = "my-prompt.md"
"#,
        )
        .unwrap();

        let client = MockApiClient::immediate_end();
        let cfg = config::Config::default();
        let result = run_recipe("test", &[], None, None, None, true, &[], &cfg, &client);
        assert!(result.is_ok());

        std::env::remove_var("ANTHROPIC_API_KEY");
    }

    #[test]
    #[serial]
    fn run_recipe_no_prompt_errors() {
        let tmp = tempfile::TempDir::new().unwrap();
        let canonical = tmp.path().canonicalize().unwrap();
        std::env::set_current_dir(&canonical).unwrap();
        std::env::set_var("HOME", canonical.to_str().unwrap());
        std::env::set_var("ANTHROPIC_API_KEY", "test-key");

        std::fs::write(
            canonical.join("taskpilot.toml"),
            r#"
[recipes.test]
output_dir = "out/"
"#,
        )
        .unwrap();

        let client = MockApiClient::immediate_end();
        let cfg = config::Config::default();
        let result = run_recipe("test", &[], None, None, None, true, &[], &cfg, &client);
        assert!(result.is_err());

        std::env::remove_var("ANTHROPIC_API_KEY");
    }

    #[test]
    #[serial]
    fn run_recipe_with_inputs_and_output() {
        let tmp = tempfile::TempDir::new().unwrap();
        let canonical = tmp.path().canonicalize().unwrap();
        std::env::set_current_dir(&canonical).unwrap();
        std::env::set_var("HOME", canonical.to_str().unwrap());
        std::env::set_var("ANTHROPIC_API_KEY", "test-key");

        std::fs::write(canonical.join("data.csv"), "a,b\n1,2").unwrap();
        std::fs::write(
            canonical.join("taskpilot.toml"),
            r#"
[recipes.test]
prompt = "analyze"
input = ["data.csv"]
output_dir = "results/"
"#,
        )
        .unwrap();

        let client = MockApiClient::immediate_end();
        let cfg = config::Config::default();
        let result = run_recipe("test", &[], None, None, None, true, &[], &cfg, &client);
        assert!(result.is_ok());

        std::env::remove_var("ANTHROPIC_API_KEY");
    }

    #[test]
    #[serial]
    fn run_recipe_with_cli_input_override() {
        let tmp = tempfile::TempDir::new().unwrap();
        let canonical = tmp.path().canonicalize().unwrap();
        std::env::set_current_dir(&canonical).unwrap();
        std::env::set_var("HOME", canonical.to_str().unwrap());
        std::env::set_var("ANTHROPIC_API_KEY", "test-key");

        std::fs::write(canonical.join("override.txt"), "data").unwrap();
        std::fs::write(
            canonical.join("taskpilot.toml"),
            r#"
[recipes.test]
prompt = "go"
input = ["original.csv"]
"#,
        )
        .unwrap();

        let client = MockApiClient::immediate_end();
        let cfg = config::Config::default();
        let cli_input = vec![canonical.join("override.txt").to_string_lossy().to_string()];
        let result = run_recipe("test", &cli_input, None, None, None, true, &[], &cfg, &client);
        assert!(result.is_ok());

        std::env::remove_var("ANTHROPIC_API_KEY");
    }

    #[test]
    #[serial]
    fn run_recipe_with_model_override() {
        let tmp = tempfile::TempDir::new().unwrap();
        let canonical = tmp.path().canonicalize().unwrap();
        std::env::set_current_dir(&canonical).unwrap();
        std::env::set_var("HOME", canonical.to_str().unwrap());
        std::env::set_var("ANTHROPIC_API_KEY", "test-key");

        std::fs::write(
            canonical.join("taskpilot.toml"),
            r#"
[recipes.test]
prompt = "go"
model = "recipe-model"
"#,
        )
        .unwrap();

        let client = MockApiClient::immediate_end();
        let cfg = config::Config {
            model: Some("config-model".into()),
            ..Default::default()
        };
        // cli_model overrides recipe and config
        let result = run_recipe(
            "test", &[], None, Some("cli-model"), None, true, &[], &cfg, &client,
        );
        assert!(result.is_ok());

        std::env::remove_var("ANTHROPIC_API_KEY");
    }

    #[test]
    #[serial]
    fn run_recipe_allow_bash_from_recipe() {
        let tmp = tempfile::TempDir::new().unwrap();
        let canonical = tmp.path().canonicalize().unwrap();
        std::env::set_current_dir(&canonical).unwrap();
        std::env::set_var("HOME", canonical.to_str().unwrap());
        std::env::set_var("ANTHROPIC_API_KEY", "test-key");

        std::fs::write(
            canonical.join("taskpilot.toml"),
            r#"
[recipes.test]
prompt = "go"
allow_bash = true
"#,
        )
        .unwrap();

        let client = MockApiClient::immediate_end();
        let cfg = config::Config::default();
        let result = run_recipe("test", &[], None, None, None, false, &[], &cfg, &client);
        assert!(result.is_ok());

        std::env::remove_var("ANTHROPIC_API_KEY");
    }

    #[test]
    #[serial]
    fn run_recipe_stream_from_config() {
        let tmp = tempfile::TempDir::new().unwrap();
        let canonical = tmp.path().canonicalize().unwrap();
        std::env::set_current_dir(&canonical).unwrap();
        std::env::set_var("HOME", canonical.to_str().unwrap());
        std::env::set_var("ANTHROPIC_API_KEY", "test-key");

        std::fs::write(
            canonical.join("taskpilot.toml"),
            r#"
[recipes.test]
prompt = "go"
"#,
        )
        .unwrap();

        let client = MockApiClient::immediate_end();
        let cfg = config::Config {
            stream: Some(false),
            ..Default::default()
        };
        let result = run_recipe("test", &[], None, None, None, false, &[], &cfg, &client);
        assert!(result.is_ok());

        std::env::remove_var("ANTHROPIC_API_KEY");
    }

    #[test]
    #[serial]
    fn run_recipe_cli_output_dir_override() {
        let tmp = tempfile::TempDir::new().unwrap();
        let canonical = tmp.path().canonicalize().unwrap();
        std::env::set_current_dir(&canonical).unwrap();
        std::env::set_var("HOME", canonical.to_str().unwrap());
        std::env::set_var("ANTHROPIC_API_KEY", "test-key");

        std::fs::write(
            canonical.join("taskpilot.toml"),
            r#"
[recipes.test]
prompt = "go"
output_dir = "original/"
"#,
        )
        .unwrap();

        let client = MockApiClient::immediate_end();
        let cfg = config::Config::default();
        let override_out = canonical.join("override-out");
        let result = run_recipe(
            "test",
            &[],
            Some(override_out.to_str().unwrap()),
            None,
            None,
            true,
            &[],
            &cfg,
            &client,
        );
        assert!(result.is_ok());
        assert!(override_out.exists());

        std::env::remove_var("ANTHROPIC_API_KEY");
    }
}
