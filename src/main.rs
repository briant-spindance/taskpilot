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
    /// Install a skill from the registry (owner/repo/skill)
    Add {
        /// Skill source (e.g. anthropics/skills/pdf)
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
            recipe: recipe_name,
            prompt,
            prompt_file,
            input,
            output_dir,
            model,
            dry_run,
            no_stream,
            skills_dir,
        } => {
            // If a recipe name is given, load it and merge with CLI flags
            let (task_prompt, final_input, final_output_dir, final_model) =
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
                            run_recipe(dep_name, &[], None, None, None, no_stream, &skills_dir)?;
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

                    (p, inp, out, mdl)
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

                    (p, input, output_dir, model)
                };

            let resolved_model = final_model.unwrap_or_else(|| "claude-sonnet-4-20250514".to_string());

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
                return Ok(());
            }

            // Create workspace
            let ws = workspace::Workspace::new()?;

            // Stage inputs
            if !final_input.is_empty() {
                ws.stage_inputs(&final_input)?;
            }

            // Run agentic loop
            runner::run(&runner::Config {
                model: resolved_model,
                prompt: task_prompt,
                skills,
                work_dir: ws.dir.to_string_lossy().to_string(),
                stream: !no_stream,
            })?;

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
        },

        Commands::Install { path } => {
            install::from_local(&path)?;
        }

        Commands::Init => {
            recipe::init()?;
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
        .unwrap_or_else(|| "claude-sonnet-4-20250514".to_string());

    let skills = skill::discover(extra_skills_dirs).context("discover skills")?;
    let ws = workspace::Workspace::new()?;
    if !final_input.is_empty() {
        ws.stage_inputs(&final_input)?;
    }

    runner::run(&runner::Config {
        model: resolved_model,
        prompt: task_prompt,
        skills,
        work_dir: ws.dir.to_string_lossy().to_string(),
        stream: !no_stream,
    })?;

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
