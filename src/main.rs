mod config;
mod install;
mod recipe;
mod registry;
mod runner;
mod skill;
mod tools;
mod workspace;

use anyhow::{Context, Result};
use clap::{Parser, Subcommand};
use colored::Colorize;
use std::fs;

#[derive(Parser)]
#[command(name = "taskpilot", about = "Execute Agent Skills as standalone agentic tasks")]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// Execute a task — either a named recipe from taskpilot.toml or with inline flags
    Run {
        /// Recipe name from taskpilot.toml (optional — use flags for ad-hoc runs)
        #[arg(value_name = "RECIPE")]
        recipe: Option<String>,

        /// Task prompt as an inline string
        #[arg(long)]
        prompt: Option<String>,

        /// Task prompt read from a file
        #[arg(long)]
        prompt_file: Option<String>,

        /// Input file(s) staged into the working directory (repeatable)
        #[arg(long)]
        input: Vec<String>,

        /// Directory where output files are written
        #[arg(long)]
        output_dir: Option<String>,

        /// Anthropic model to use
        #[arg(long)]
        model: Option<String>,

        /// Print resolved config without executing
        #[arg(long)]
        dry_run: bool,

        /// Disable streaming output (wait for full response)
        #[arg(long)]
        no_stream: bool,

        /// Additional skills directory to search (repeatable)
        #[arg(long = "skills-dir")]
        skills_dir: Vec<String>,

        /// Allow the bash tool (shell command execution is disabled by default)
        #[arg(long)]
        allow_bash: bool,
    },

    /// List recipes defined in taskpilot.toml
    Recipes,

    /// Validate taskpilot.toml and check environment
    Doctor,

    /// Manage and inspect discovered skills
    Skills {
        #[command(subcommand)]
        action: SkillsAction,
    },

    /// Initialize a new taskpilot.toml with an example recipe
    Init,

    /// Set up global config (~/.local/taskpilot/config.yml)
    Config,

}

#[derive(Subcommand)]
enum SkillsAction {
    /// List all discovered skills
    List,
    /// Show details for a specific skill
    Show {
        /// Skill name
        name: String,
    },
    /// Search the skills.sh registry
    Find {
        /// Search query
        query: Vec<String>,
    },
    /// Install a skill from the registry (owner/repo/skill)
    Add {
        /// Skill source (e.g. anthropics/skills/pdf)
        source: String,

        /// Install globally (~/.agents/skills/) instead of project-level
        #[arg(short, long)]
        global: bool,
    },
    /// Install a skill from a local directory
    Install {
        /// Path to the skill directory
        path: String,
    },
}

fn main() -> Result<()> {
    // Load project-level .env if present
    let _ = dotenvy::dotenv();

    // Load global config from ~/.local/taskpilot/config.yml
    let global_config = config::load();

    // If ANTHROPIC_API_KEY is not set via env/.env, use config.yml value
    if std::env::var("ANTHROPIC_API_KEY").is_err() {
        if let Some(ref key) = global_config.api_key {
            std::env::set_var("ANTHROPIC_API_KEY", key);
        }
    }

    let cli = Cli::parse();
    dispatch_command(cli.command, &global_config, &runner::DefaultApiClient)
}

/// Core command dispatch, parameterized on API client for testability.
fn dispatch_command(
    command: Commands,
    global_config: &config::Config,
    api_client: &dyn runner::ApiClient,
) -> Result<()> {
    match command {
        Commands::Run {
            recipe: recipe_name,
            prompt,
            prompt_file,
            input,
            output_dir,
            model,
            dry_run,
            no_stream,
            skills_dir,
            allow_bash: cli_allow_bash,
        } => {
            // If a recipe name is given, load it and merge with CLI flags
            let (task_prompt, final_input, final_output_dir, final_model, recipe_allow_bash) =
                if let Some(ref name) = recipe_name {
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
                            run_recipe(dep_name, &[], None, None, None, no_stream, &skills_dir, &global_config, api_client)?;
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

                    // CLI flags override recipe values
                    let p = if let Some(ref pf) = prompt_file {
                        fs::read_to_string(pf)
                            .with_context(|| format!("read prompt file: {pf}"))?
                    } else if let Some(ref p) = prompt {
                        p.clone()
                    } else if let Some(ref pf) = r.prompt_file {
                        fs::read_to_string(pf)
                            .with_context(|| format!("read prompt file: {pf}"))?
                    } else if let Some(ref p) = r.prompt {
                        p.clone()
                    } else {
                        anyhow::bail!("recipe {name:?} has no prompt or prompt_file");
                    };

                    let inp = if !input.is_empty() { input } else { r.input.clone() };
                    let out = output_dir.or(r.output_dir.clone());
                    let mdl = model.or(r.model.clone());

                    (p, inp, out, mdl, r.allow_bash)
                } else {
                    // No recipe — pure flag-based run
                    let p = if let Some(pf) = prompt_file {
                        fs::read_to_string(&pf)
                            .with_context(|| format!("read prompt file: {pf}"))?
                    } else if let Some(p) = prompt {
                        p
                    } else {
                        anyhow::bail!("--prompt or --prompt-file is required (or use a recipe name)");
                    };

                    (p, input, output_dir, model, None)
                };

            let resolved_model = final_model
                .or(global_config.model.clone())
                .unwrap_or_else(|| "claude-sonnet-4-20250514".to_string());

            let use_stream = if no_stream {
                false
            } else {
                global_config.stream.unwrap_or(true)
            };

            // Resolve allow_bash: CLI flag > recipe field > config.yml > default (false)
            let allow_bash = if cli_allow_bash {
                true
            } else {
                recipe_allow_bash
                    .or(global_config.allow_bash)
                    .unwrap_or(false)
            };

            // Discover skills
            let skills = skill::discover(&skills_dir).context("discover skills")?;

            if dry_run {
                println!("=== Dry Run ===");
                if let Some(ref name) = recipe_name {
                    println!("Recipe: {name}");
                }
                println!("Model: {resolved_model}");
                println!("Prompt: {task_prompt}");
                println!("Skills: {} discovered", skills.len());
                for s in &skills {
                    println!("  - {} ({})", s.name, s.path.display());
                }
                println!("Inputs: {final_input:?}");
                println!("Output dir: {}", final_output_dir.as_deref().unwrap_or("(none)"));
                println!("Bash: {}", if allow_bash { "enabled" } else { "disabled" });
                return Ok(());
            }

            // Create workspace
            let ws = workspace::Workspace::new()?;

            // Stage inputs
            if !final_input.is_empty() {
                ws.stage_inputs(&final_input)?;
            }

            // Run agentic loop
            runner::run_with_client(&runner::Config {
                model: resolved_model,
                prompt: task_prompt,
                skills,
                work_dir: ws.dir.to_string_lossy().to_string(),
                stream: use_stream,
                allow_bash,
            }, api_client)?;

            // Collect outputs
            if let Some(ref out) = final_output_dir {
                ws.collect_outputs(out)?;
            }
        }

        Commands::Recipes => {
            recipe::list()?;
        }

        Commands::Doctor => {
            recipe::doctor()?;
        }

        Commands::Skills { action } => match action {
            SkillsAction::List => {
                let skills = skill::discover(&[])?;
                if skills.is_empty() {
                    println!("{}", "No skills found.".dimmed());
                } else {
                    for (i, s) in skills.iter().enumerate() {
                        if i > 0 {
                            println!();
                        }
                        let desc = if s.description.is_empty() {
                            "(no description)".dimmed().to_string()
                        } else {
                            truncate_desc(&s.description, 80).dimmed().to_string()
                        };
                        println!("  {} {}", "●".green(), s.name.bold());
                        println!("    {}", s.path.display().to_string().dimmed());
                        println!("    {desc}");
                    }
                }
            }
            SkillsAction::Show { name } => {
                let skills = skill::discover(&[])?;
                let s = skill::find_by_name(&skills, &name)?;
                println!("Name:        {}", s.name);
                println!("Description: {}", s.description);
                println!("Path:        {}", s.path.display());
            }
            SkillsAction::Find { query } => {
                let q = query.join(" ");
                if q.is_empty() {
                    anyhow::bail!("search query is required");
                }
                registry::find(&q)?;
            }
            SkillsAction::Add { source, global } => {
                registry::add(&source, global)?;
            }
            SkillsAction::Install { path } => {
                install::from_local(&path)?;
            }
        },

        Commands::Init => {
            recipe::init()?;
        }

        Commands::Config => {
            config::setup()?;
        }
    }

    Ok(())
}

/// Run a single recipe by name (used for dependency execution).
fn run_recipe(
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

    let task_prompt = if let Some(p) = cli_prompt {
        p.to_string()
    } else if let Some(ref pf) = r.prompt_file {
        fs::read_to_string(pf).with_context(|| format!("read prompt file: {pf}"))?
    } else if let Some(ref p) = r.prompt {
        p.clone()
    } else {
        anyhow::bail!("recipe {name:?} has no prompt or prompt_file");
    };

    let final_input = if !cli_input.is_empty() {
        cli_input.to_vec()
    } else {
        r.input.clone()
    };
    let final_output_dir = cli_output_dir
        .map(|s| s.to_string())
        .or(r.output_dir.clone());
    let resolved_model = cli_model
        .map(|s| s.to_string())
        .or(r.model.clone())
        .or(global_config.model.clone())
        .unwrap_or_else(|| "claude-sonnet-4-20250514".to_string());

    let use_stream = if no_stream {
        false
    } else {
        global_config.stream.unwrap_or(true)
    };

    let allow_bash = r.allow_bash
        .or(global_config.allow_bash)
        .unwrap_or(false);

    let skills = skill::discover(extra_skills_dirs).context("discover skills")?;
    let ws = workspace::Workspace::new()?;
    if !final_input.is_empty() {
        ws.stage_inputs(&final_input)?;
    }

    runner::run_with_client(&runner::Config {
        model: resolved_model,
        prompt: task_prompt,
        skills,
        work_dir: ws.dir.to_string_lossy().to_string(),
        stream: use_stream,
        allow_bash,
    }, api_client)?;

    if let Some(ref out) = final_output_dir {
        ws.collect_outputs(out)?;
    }

    Ok(())
}

fn truncate_desc(s: &str, max: usize) -> String {
    let first_sentence = s
        .split_once(". ")
        .map(|(f, _)| format!("{f}."))
        .unwrap_or_else(|| s.to_string());
    if first_sentence.len() <= max {
        first_sentence
    } else {
        format!("{}...", &first_sentence[..max])
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serial_test::serial;

    #[test]
    fn truncate_desc_first_sentence_fits() {
        assert_eq!(
            truncate_desc("Hello world. More text here", 20),
            "Hello world."
        );
    }

    #[test]
    fn truncate_desc_first_sentence_exceeds_max() {
        assert_eq!(
            truncate_desc("This is a long first sentence. And more.", 10),
            "This is a ..."
        );
    }

    #[test]
    fn truncate_desc_no_separator_fits() {
        assert_eq!(truncate_desc("Short text", 20), "Short text");
    }

    #[test]
    fn truncate_desc_no_separator_exceeds_max() {
        assert_eq!(truncate_desc("A longer string here", 7), "A longe...");
    }

    #[test]
    fn truncate_desc_empty_string() {
        assert_eq!(truncate_desc("", 10), "");
    }

    #[test]
    fn truncate_desc_exactly_at_max() {
        assert_eq!(truncate_desc("12345", 5), "12345");
    }

    // ── MockApiClient for main.rs tests ──────────────────────────

    use std::cell::RefCell;
    use std::collections::VecDeque;

    struct MockApiClient {
        responses: RefCell<VecDeque<anyhow::Result<(serde_json::Value, String)>>>,
    }

    impl MockApiClient {
        fn immediate_end() -> Self {
            Self {
                responses: RefCell::new(VecDeque::from(vec![Ok((
                    serde_json::json!([{"type": "text", "text": "done"}]),
                    "end_turn".into(),
                ))])),
            }
        }
    }

    impl runner::ApiClient for MockApiClient {
        fn call(
            &self,
            _api_key: &str,
            _body: &serde_json::Value,
            _stream: bool,
            _iteration: usize,
        ) -> anyhow::Result<(serde_json::Value, String)> {
            self.responses.borrow_mut().pop_front().unwrap()
        }
    }

    // ── run_recipe tests ─────────────────────────────────────────

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
        let cfg = config::Config { model: Some("config-model".into()), ..Default::default() };
        // cli_model overrides recipe and config
        let result = run_recipe("test", &[], None, Some("cli-model"), None, true, &[], &cfg, &client);
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
        let cfg = config::Config { stream: Some(false), ..Default::default() };
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
            "test", &[], Some(override_out.to_str().unwrap()), None, None, true, &[], &cfg, &client,
        );
        assert!(result.is_ok());
        assert!(override_out.exists());

        std::env::remove_var("ANTHROPIC_API_KEY");
    }

    // ── dispatch_command tests ───────────────────────────────────

    fn setup_dispatch_env(tmp: &tempfile::TempDir) -> std::path::PathBuf {
        let canonical = tmp.path().canonicalize().unwrap();
        std::env::set_current_dir(&canonical).unwrap();
        std::env::set_var("HOME", canonical.to_str().unwrap());
        std::env::set_var("ANTHROPIC_API_KEY", "test-key");
        canonical
    }

    fn teardown_dispatch_env() {
        std::env::remove_var("ANTHROPIC_API_KEY");
    }

    #[test]
    #[serial]
    fn dispatch_run_no_recipe_with_prompt_executes() {
        let tmp = tempfile::TempDir::new().unwrap();
        let dir = setup_dispatch_env(&tmp);
        std::fs::write(dir.join("input.txt"), "data").unwrap();
        let client = MockApiClient::immediate_end();
        let cfg = config::Config::default();

        let result = dispatch_command(
            Commands::Run {
                recipe: None,
                prompt: Some("do it".into()),
                prompt_file: None,
                input: vec![dir.join("input.txt").to_string_lossy().to_string()],
                output_dir: Some(dir.join("out").to_string_lossy().to_string()),
                model: None,
                dry_run: false,
                no_stream: true,
                skills_dir: vec![],
                allow_bash: false,
            },
            &cfg,
            &client,
        );
        assert!(result.is_ok());
        assert!(dir.join("out").exists());
        teardown_dispatch_env();
    }

    #[test]
    #[serial]
    fn dispatch_run_no_recipe_with_prompt_file() {
        let tmp = tempfile::TempDir::new().unwrap();
        let dir = setup_dispatch_env(&tmp);
        std::fs::write(dir.join("p.txt"), "my prompt").unwrap();
        let client = MockApiClient::immediate_end();
        let cfg = config::Config::default();

        let result = dispatch_command(
            Commands::Run {
                recipe: None,
                prompt: None,
                prompt_file: Some("p.txt".into()),
                input: vec![],
                output_dir: None,
                model: Some("test-model".into()),
                dry_run: false,
                no_stream: false,
                skills_dir: vec![],
                allow_bash: true,
            },
            &cfg,
            &client,
        );
        assert!(result.is_ok());
        teardown_dispatch_env();
    }

    #[test]
    #[serial]
    fn dispatch_run_recipe_with_skill_deps_installed() {
        let tmp = tempfile::TempDir::new().unwrap();
        let dir = setup_dispatch_env(&tmp);

        // Create installed skill
        let skill_dir = dir.join(".agents").join("skills").join("myskill");
        std::fs::create_dir_all(&skill_dir).unwrap();
        std::fs::write(skill_dir.join("SKILL.md"), "# Skill").unwrap();

        std::fs::write(
            dir.join("taskpilot.toml"),
            r#"
[recipes.test]
prompt = "go"
skill_deps = ["myskill"]
"#,
        )
        .unwrap();

        let client = MockApiClient::immediate_end();
        let cfg = config::Config::default();

        let result = dispatch_command(
            Commands::Run {
                recipe: Some("test".into()),
                prompt: None,
                prompt_file: None,
                input: vec![],
                output_dir: None,
                model: None,
                dry_run: false,
                no_stream: true,
                skills_dir: vec![],
                allow_bash: false,
            },
            &cfg,
            &client,
        );
        assert!(result.is_ok());
        teardown_dispatch_env();
    }

    #[test]
    #[serial]
    fn dispatch_run_recipe_with_input_override() {
        let tmp = tempfile::TempDir::new().unwrap();
        let dir = setup_dispatch_env(&tmp);

        std::fs::write(dir.join("override.txt"), "override data").unwrap();
        std::fs::write(
            dir.join("taskpilot.toml"),
            r#"
[recipes.test]
prompt = "go"
input = ["missing.csv"]
output_dir = "out/"
"#,
        )
        .unwrap();

        let client = MockApiClient::immediate_end();
        let cfg = config::Config::default();

        let result = dispatch_command(
            Commands::Run {
                recipe: Some("test".into()),
                prompt: None,
                prompt_file: None,
                input: vec![dir.join("override.txt").to_string_lossy().to_string()],
                output_dir: Some(dir.join("result").to_string_lossy().to_string()),
                model: None,
                dry_run: false,
                no_stream: true,
                skills_dir: vec![],
                allow_bash: false,
            },
            &cfg,
            &client,
        );
        assert!(result.is_ok());
        teardown_dispatch_env();
    }

    #[test]
    #[serial]
    fn dispatch_run_recipe_with_deps_executes() {
        let tmp = tempfile::TempDir::new().unwrap();
        let dir = setup_dispatch_env(&tmp);

        std::fs::write(
            dir.join("taskpilot.toml"),
            r#"
[recipes.step1]
prompt = "first"

[recipes.step2]
prompt = "second"
depends_on = ["step1"]
"#,
        )
        .unwrap();

        let client = MockApiClient {
            responses: RefCell::new(VecDeque::from(vec![
                Ok((serde_json::json!([{"type":"text","text":"ok"}]), "end_turn".into())),
                Ok((serde_json::json!([{"type":"text","text":"ok"}]), "end_turn".into())),
            ])),
        };
        let cfg = config::Config::default();

        let result = dispatch_command(
            Commands::Run {
                recipe: Some("step2".into()),
                prompt: None,
                prompt_file: None,
                input: vec![],
                output_dir: None,
                model: None,
                dry_run: false,
                no_stream: true,
                skills_dir: vec![],
                allow_bash: false,
            },
            &cfg,
            &client,
        );
        assert!(result.is_ok());
        teardown_dispatch_env();
    }

    #[test]
    #[serial]
    fn dispatch_run_recipe_allow_bash_from_cli() {
        let tmp = tempfile::TempDir::new().unwrap();
        let dir = setup_dispatch_env(&tmp);

        std::fs::write(
            dir.join("taskpilot.toml"),
            r#"
[recipes.test]
prompt = "go"
"#,
        )
        .unwrap();

        let client = MockApiClient::immediate_end();
        let cfg = config::Config::default();

        let result = dispatch_command(
            Commands::Run {
                recipe: Some("test".into()),
                prompt: None,
                prompt_file: None,
                input: vec![],
                output_dir: None,
                model: None,
                dry_run: false,
                no_stream: true,
                skills_dir: vec![],
                allow_bash: true,
            },
            &cfg,
            &client,
        );
        assert!(result.is_ok());
        teardown_dispatch_env();
    }

    #[test]
    #[serial]
    fn dispatch_skills_list_multiple() {
        let tmp = tempfile::TempDir::new().unwrap();
        let dir = setup_dispatch_env(&tmp);

        // Create two skills
        for name in &["skill-a", "skill-b"] {
            let sd = dir.join(".agents").join("skills").join(name);
            std::fs::create_dir_all(&sd).unwrap();
            std::fs::write(sd.join("SKILL.md"), format!("---\nname: {name}\ndescription: Desc for {name}\n---\n")).unwrap();
        }

        let client = MockApiClient::immediate_end();
        let cfg = config::Config::default();

        let result = dispatch_command(
            Commands::Skills { action: SkillsAction::List },
            &cfg,
            &client,
        );
        assert!(result.is_ok());
        teardown_dispatch_env();
    }

    #[test]
    #[serial]
    fn dispatch_run_no_prompt_no_prompt_file_errors() {
        let tmp = tempfile::TempDir::new().unwrap();
        setup_dispatch_env(&tmp);
        let client = MockApiClient::immediate_end();
        let cfg = config::Config::default();

        let result = dispatch_command(
            Commands::Run {
                recipe: None,
                prompt: None,
                prompt_file: None,
                input: vec![],
                output_dir: None,
                model: None,
                dry_run: false,
                no_stream: true,
                skills_dir: vec![],
                allow_bash: false,
            },
            &cfg,
            &client,
        );
        assert!(result.is_err());
        teardown_dispatch_env();
    }
}
