mod install;
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
    /// Execute a skill against a prompt
    Run {
        /// Specific skill to use (optional; model selects from catalog if omitted)
        #[arg(long)]
        skill: Option<String>,

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
        output: Option<String>,

        /// Anthropic model to use
        #[arg(long, default_value = "claude-sonnet-4-20250514")]
        model: String,

        /// Print resolved config without executing
        #[arg(long)]
        dry_run: bool,
    },

    /// Manage and inspect discovered skills
    Skills {
        #[command(subcommand)]
        action: SkillsAction,
    },

    /// Install a skill from a local directory
    Install {
        /// Path to the skill directory
        path: String,
    },
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
    /// Install a skill from the registry (owner/repo@skill)
    Add {
        /// Skill source (e.g. anthropics/skills@pdf)
        source: String,

        /// Install globally (~/.agents/skills/) instead of project-level
        #[arg(short, long)]
        global: bool,
    },
}

fn main() -> Result<()> {
    // Load .env file if present (silently ignore if missing)
    let _ = dotenvy::dotenv();

    let cli = Cli::parse();

    match cli.command {
        Commands::Run {
            skill: skill_name,
            prompt,
            prompt_file,
            input,
            output,
            model,
            dry_run,
        } => {
            // Resolve prompt
            let task_prompt = if let Some(pf) = prompt_file {
                fs::read_to_string(&pf).with_context(|| format!("read prompt file: {pf}"))?
            } else if let Some(p) = prompt {
                p
            } else {
                anyhow::bail!("--prompt or --prompt-file is required");
            };

            // Discover skills
            let mut skills = skill::discover().context("discover skills")?;

            // Filter to specific skill if requested
            if let Some(ref name) = skill_name {
                let s = skill::find_by_name(&skills, name)?.clone();
                skills = vec![s];
            }

            if dry_run {
                println!("=== Dry Run ===");
                println!("Model: {model}");
                println!("Prompt: {task_prompt}");
                println!("Skills: {} discovered", skills.len());
                for s in &skills {
                    println!("  - {} ({})", s.name, s.path.display());
                }
                println!("Inputs: {input:?}");
                println!("Output: {}", output.as_deref().unwrap_or("(none)"));
                return Ok(());
            }

            // Create workspace
            let ws = workspace::Workspace::new()?;

            // Stage inputs
            if !input.is_empty() {
                ws.stage_inputs(&input)?;
            }

            // Run agentic loop
            runner::run(&runner::Config {
                model,
                prompt: task_prompt,
                skills,
                work_dir: ws.dir.to_string_lossy().to_string(),
            })?;

            // Collect outputs
            if let Some(ref out) = output {
                ws.collect_outputs(out)?;
            }
        }

        Commands::Skills { action } => match action {
            SkillsAction::List => {
                let skills = skill::discover()?;
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
                let skills = skill::discover()?;
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
        },

        Commands::Install { path } => {
            install::from_local(&path)?;
        }
    }

    Ok(())
}

fn truncate_desc(s: &str, max: usize) -> String {
    // Take first sentence or max chars, whichever is shorter
    let first_sentence = s.split_once(". ").map(|(f, _)| format!("{f}.")).unwrap_or_else(|| s.to_string());
    if first_sentence.len() <= max {
        first_sentence
    } else {
        format!("{}...", &first_sentence[..max])
    }
}
