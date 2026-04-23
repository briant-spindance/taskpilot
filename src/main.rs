mod config;
mod constants;
mod pipeline;
mod recipe;
mod registry;
mod runner;
mod skill;
mod tools;
mod workspace;

#[cfg(test)]
mod testutil;

use anyhow::Result;
use clap::{Parser, Subcommand};
use colored::Colorize;

#[derive(Parser)]
#[command(name = "taskpilot", about = "Execute Agent Skills as standalone agentic tasks", version)]
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

    /// Print version information
    Version,
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
            recipe,
            prompt,
            prompt_file,
            input,
            output_dir,
            model,
            dry_run,
            no_stream,
            skills_dir,
            allow_bash,
        } => {
            pipeline::run_command(
                recipe, prompt, prompt_file, input, output_dir, model, dry_run, no_stream,
                skills_dir, allow_bash, global_config, api_client,
            )?;
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
                registry::from_local(&path)?;
            }
        },

        Commands::Init => {
            recipe::init()?;
        }

        Commands::Config => {
            config::setup()?;
        }

        Commands::Version => {
            println!("taskpilot {}", env!("CARGO_PKG_VERSION"));
        }
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

    // ── MockApiClient for dispatch_command tests ──────────────────

    use crate::testutil::MockApiClient;
    use std::cell::RefCell;
    use std::collections::VecDeque;

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
        let _dir = setup_dispatch_env(&tmp);

        std::fs::write(
            tmp.path().canonicalize().unwrap().join("taskpilot.toml"),
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
