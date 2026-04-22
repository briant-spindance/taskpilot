use anyhow::{bail, Context, Result};
use colored::Colorize;
use serde::Deserialize;
use std::collections::HashMap;
use std::fs;
use std::io::{self, Write};
use std::path::Path;

use crate::registry;
use crate::skill;

const RECIPE_FILE: &str = "taskpilot.toml";

#[derive(Debug, Deserialize)]
struct RecipeFile {
    #[serde(default)]
    recipes: HashMap<String, Recipe>,
}

#[derive(Debug, Deserialize, Clone)]
pub struct Recipe {
    pub prompt: Option<String>,
    pub prompt_file: Option<String>,
    #[serde(default)]
    pub input: Vec<String>,
    pub output_dir: Option<String>,
    pub model: Option<String>,
    #[serde(default)]
    pub skill_deps: Vec<String>,
}

/// Load and parse the taskpilot.toml file from the current directory.
pub fn load() -> Result<HashMap<String, Recipe>> {
    let path = Path::new(RECIPE_FILE);
    if !path.exists() {
        bail!("no {RECIPE_FILE} found in the current directory");
    }
    let content = fs::read_to_string(path).context("read taskpilot.toml")?;
    let file: RecipeFile = toml::from_str(&content).context("parse taskpilot.toml")?;
    Ok(file.recipes)
}

/// Look up a recipe by name.
pub fn get(name: &str) -> Result<Recipe> {
    let recipes = load()?;
    recipes
        .get(name)
        .cloned()
        .with_context(|| {
            let available: Vec<_> = recipes.keys().collect();
            format!(
                "recipe {name:?} not found in {RECIPE_FILE}. Available: {}",
                if available.is_empty() {
                    "(none)".to_string()
                } else {
                    available.iter().map(|s| s.as_str()).collect::<Vec<_>>().join(", ")
                }
            )
        })
}

/// List all recipes with a summary.
pub fn list() -> Result<()> {
    let recipes = load()?;
    if recipes.is_empty() {
        println!("{}", "No recipes defined in taskpilot.toml.".dimmed());
        return Ok(());
    }

    for (name, recipe) in &recipes {
        let summary = if let Some(ref pf) = recipe.prompt_file {
            format!("prompt-file: {pf}")
        } else if let Some(ref p) = recipe.prompt {
            let truncated = if p.len() > 60 { format!("{}...", &p[..60]) } else { p.clone() };
            truncated
        } else {
            "(no prompt)".to_string()
        };

        println!("  {} {}", "●".green(), name.bold());

        let mut details = Vec::new();
        if !recipe.input.is_empty() {
            details.push(format!("{} input(s)", recipe.input.len()));
        }
        if let Some(ref out) = recipe.output_dir {
            details.push(format!("→ {out}"));
        }
        if !recipe.skill_deps.is_empty() {
            details.push(format!("{} skill dep(s)", recipe.skill_deps.len()));
        }
        if !details.is_empty() {
            println!("    {}", details.join(" · ").dimmed());
        }
        println!("    {}", summary.dimmed());
    }

    Ok(())
}

/// Validate taskpilot.toml and check the environment.
pub fn doctor() -> Result<()> {
    let mut errors = 0u32;
    let mut warnings = 0u32;

    println!("{}", "taskpilot doctor".bold());
    println!();

    // 1. Check .env / ANTHROPIC_API_KEY
    print!("  ANTHROPIC_API_KEY  ");
    if std::env::var("ANTHROPIC_API_KEY").is_ok() {
        println!("{}", "✓ set".green());
    } else {
        println!("{}", "✗ not set".red());
        errors += 1;
    }

    // 2. Check taskpilot.toml exists
    let path = Path::new(RECIPE_FILE);
    print!("  taskpilot.toml     ");
    if !path.exists() {
        println!("{}", "- not found (optional)".dimmed());
        println!();
        print_summary(errors, warnings);
        return Ok(());
    }
    println!("{}", "✓ found".green());

    // 3. Parse TOML
    let content = fs::read_to_string(path).context("read taskpilot.toml")?;
    print!("  TOML syntax        ");
    let file: RecipeFile = match toml::from_str(&content) {
        Ok(f) => {
            println!("{}", "✓ valid".green());
            f
        }
        Err(e) => {
            println!("{}", "✗ invalid".red());
            println!("    {}", e.to_string().dimmed());
            errors += 1;
            println!();
            print_summary(errors, warnings);
            return Ok(());
        }
    };

    // 4. Check each recipe
    print!("  Recipes            ");
    if file.recipes.is_empty() {
        println!("{}", "- none defined".dimmed());
    } else {
        println!("{} defined", file.recipes.len().to_string().bold());
    }
    println!();

    let skills = skill::discover().unwrap_or_default();

    for (name, recipe) in &file.recipes {
        println!("  {} {}", "●".cyan(), name.bold());

        // Check prompt
        if recipe.prompt.is_none() && recipe.prompt_file.is_none() {
            println!("    {} no prompt or prompt_file", "✗".red());
            errors += 1;
        } else if let Some(ref pf) = recipe.prompt_file {
            if Path::new(pf).exists() {
                println!("    {} prompt_file: {pf}", "✓".green());
            } else {
                println!("    {} prompt_file not found: {pf}", "✗".red());
                errors += 1;
            }
        } else {
            println!("    {} prompt: inline", "✓".green());
        }

        // Check inputs exist
        for inp in &recipe.input {
            if Path::new(inp).exists() {
                println!("    {} input: {inp}", "✓".green());
            } else {
                println!("    {} input not found: {inp}", "✗".red());
                errors += 1;
            }
        }

        // Check output_dir
        if let Some(ref out) = recipe.output_dir {
            println!("    {} output_dir: {out}", "✓".green());
        } else {
            println!("    {} no output_dir (files stay in workspace)", "!".yellow());
            warnings += 1;
        }

        // Check skill deps
        for dep in &recipe.skill_deps {
            let is_remote = dep.contains('/');
            let skill_name = if is_remote {
                dep.rsplit('/').next().unwrap_or(dep)
            } else {
                dep.as_str()
            };

            if skill::find_by_name(&skills, skill_name).is_ok() {
                println!("    {} skill_dep: {dep}", "✓".green());
            } else if is_remote {
                println!(
                    "    {} skill_dep: {dep} {}",
                    "!".yellow(),
                    "(not installed — will prompt on run)".dimmed()
                );
                warnings += 1;
            } else {
                println!("    {} skill_dep: {dep} (not installed)", "✗".red());
                errors += 1;
            }
        }

        println!();
    }

    print_summary(errors, warnings);
    Ok(())
}

fn print_summary(errors: u32, warnings: u32) {
    if errors == 0 && warnings == 0 {
        println!("{}", "All checks passed.".green().bold());
    } else {
        if errors > 0 {
            print!("{} ", format!("{errors} error(s)").red().bold());
        }
        if warnings > 0 {
            print!("{} ", format!("{warnings} warning(s)").yellow().bold());
        }
        println!();
    }
}

/// Check and resolve skill dependencies for a recipe.
/// Local deps (bare names) are verified. Remote deps (with /) are auto-installed if missing.
pub fn resolve_skill_deps(deps: &[String]) -> Result<()> {
    if deps.is_empty() {
        return Ok(());
    }

    let skills = skill::discover().context("discover skills for dep check")?;

    for dep in deps {
        let is_remote = dep.contains('/');
        let skill_name = if is_remote {
            // Last segment is the skill name
            dep.rsplit('/').next().unwrap_or(dep)
        } else {
            dep.as_str()
        };

        // Check if already installed
        if skill::find_by_name(&skills, skill_name).is_ok() {
            eprintln!(
                "  {} skill {} {}",
                "✓".green(),
                skill_name.bold(),
                "installed".dimmed()
            );
            continue;
        }

        if is_remote {
            // Ask user where to install
            eprintln!(
                "\n  {} skill {} is required but not installed.",
                "!".yellow().bold(),
                skill_name.bold()
            );
            eprintln!("    Source: {}", dep.dimmed());
            eprintln!();
            eprintln!("    Where should it be installed?");
            eprintln!("      {} Install globally (~/.agents/skills/)", "[g]".bold());
            eprintln!("      {} Install locally (./.agents/skills/)", "[l]".bold());
            eprintln!("      {} Cancel", "[c]".bold());
            eprint!("    > ");
            io::stderr().flush()?;

            let mut input = String::new();
            io::stdin().read_line(&mut input)?;
            let choice = input.trim().to_lowercase();

            match choice.as_str() {
                "g" | "global" => {
                    registry::add(dep, true)?;
                }
                "l" | "local" => {
                    registry::add(dep, false)?;
                }
                _ => {
                    bail!("installation cancelled — recipe requires skill {skill_name:?}");
                }
            }
        } else {
            // Local-only dep, can't auto-install
            bail!(
                "recipe requires skill {skill_name:?} which is not installed.\n  \
                 Search: taskpilot skills find {skill_name}\n  \
                 Add:    taskpilot skills add <owner/repo/{skill_name}>"
            );
        }
    }

    Ok(())
}
