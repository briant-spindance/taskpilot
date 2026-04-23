use anyhow::{bail, Context, Result};
use colored::Colorize;
use serde::Deserialize;
use std::collections::{HashMap, HashSet};
use std::fs;
use std::io::{self, BufRead, Write};
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
pub(crate) struct Recipe {
    pub(crate) description: Option<String>,
    pub(crate) prompt: Option<String>,
    pub(crate) prompt_file: Option<String>,
    #[serde(default)]
    pub(crate) input: Vec<String>,
    pub(crate) output_dir: Option<String>,
    pub(crate) model: Option<String>,
    #[serde(default)]
    pub(crate) skill_deps: Vec<String>,
    #[serde(default)]
    pub(crate) depends_on: Vec<String>,
    pub(crate) allow_bash: Option<bool>,
}

/// Load and parse the taskpilot.toml file from the current directory.
pub(crate) fn load() -> Result<HashMap<String, Recipe>> {
    let path = Path::new(RECIPE_FILE);
    if !path.exists() {
        bail!("no {RECIPE_FILE} found in the current directory");
    }
    let content = fs::read_to_string(path).context("read taskpilot.toml")?;
    let file: RecipeFile = toml::from_str(&content).context("parse taskpilot.toml")?;
    Ok(file.recipes)
}

/// Look up a recipe by name.
pub(crate) fn get(name: &str) -> Result<Recipe> {
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
pub(crate) fn list() -> Result<()> {
    let recipes = load()?;
    if recipes.is_empty() {
        println!("{}", "No recipes defined in taskpilot.toml.".dimmed());
        return Ok(());
    }

    for (name, recipe) in &recipes {
        let summary = if let Some(ref pf) = recipe.prompt_file {
            format!("prompt-file: {pf}")
        } else if let Some(ref p) = recipe.prompt {
            let collapsed = p.replace('\n', " ");
            let trimmed = collapsed.trim();
            if trimmed.len() > 60 { format!("{}...", &trimmed[..60]) } else { trimmed.to_string() }
        } else {
            "(no prompt)".to_string()
        };

        println!("  {} {}", "●".green(), name.bold());
        if let Some(ref desc) = recipe.description {
            println!("    {}", desc.dimmed());
        }

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

/// Initialize a new taskpilot.toml with an example recipe.
pub(crate) fn init() -> Result<()> {
    let path = Path::new(RECIPE_FILE);
    if path.exists() {
        bail!("{RECIPE_FILE} already exists in the current directory");
    }

    let template = r#"# taskpilot.toml — define named recipes for repeatable agentic tasks
#
# Run a recipe:   taskpilot run <name>
# List recipes:   taskpilot recipes
# Validate:       taskpilot doctor
#
# Recipes can depend on other recipes via depends_on. Dependencies
# run first in topological order. Circular dependencies are rejected.

[recipes.prepare-data]
description = "Remove incomplete rows from raw CSV data"
prompt = """
Read input.csv, clean missing values,
and write cleaned.csv
"""
input = ["data/input.csv"]
output_dir = "staging/"

[recipes.generate-report]
description = "Analyze cleaned data and produce a summary report"
prompt = """
Analyze cleaned.csv and produce a summary report
in report.md with key metrics and insights
"""
input = ["staging/cleaned.csv"]
output_dir = "output/"
model = "claude-sonnet-4-20250514"
allow_bash = true
# skill_deps = ["markdown-report"]
depends_on = ["prepare-data"]
"#;

    fs::write(path, template).context("write taskpilot.toml")?;
    println!("{} Created {RECIPE_FILE}", "✓".green());
    println!("  Edit the file to define your recipes, then run: taskpilot doctor");
    Ok(())
}

/// Validate taskpilot.toml and check the environment.
pub(crate) fn doctor() -> Result<()> {
    let mut errors = 0u32;
    let mut warnings = 0u32;

    println!("{}", "taskpilot doctor".bold());
    println!();

    // 1. Check config.yml
    let config_path = crate::config::path_display();
    print!("  config.yml         ");
    if std::path::Path::new(&config_path).exists() {
        println!("{}", "✓ found".green());
    } else {
        println!("{}", format!("- not found ({config_path})").dimmed());
    }

    // 2. Check ANTHROPIC_API_KEY
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

    let skills = skill::discover(&[]).unwrap_or_default();

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

        // Check depends_on references
        for dep in &recipe.depends_on {
            if file.recipes.contains_key(dep.as_str()) {
                println!("    {} depends_on: {dep}", "✓".green());
            } else {
                println!("    {} depends_on: {dep} (recipe not found)", "✗".red());
                errors += 1;
            }
        }

        // Check for circular dependencies
        if !recipe.depends_on.is_empty() {
            match resolve_depends_on(name) {
                Ok(_) => {}
                Err(e) => {
                    println!("    {} {e}", "✗".red());
                    errors += 1;
                }
            }
        }

        // Show allow_bash status
        if recipe.allow_bash.unwrap_or(false) {
            println!("    {} bash: enabled", "!".yellow());
        } else {
            println!("    {} bash: disabled", "✓".green());
        }

        println!();
    }

    print_summary(errors, warnings);
    Ok(())
}

pub(crate) fn print_summary(errors: u32, warnings: u32) {
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

/// Resolve the full execution order for a recipe, honoring depends_on.
/// Returns a topologically sorted list of recipe names ending with the target.
pub(crate) fn resolve_depends_on(target: &str) -> Result<Vec<String>> {
    let recipes = load()?;

    // Check target exists
    if !recipes.contains_key(target) {
        bail!("recipe {target:?} not found in {RECIPE_FILE}");
    }

    let mut order = Vec::new();
    let mut visited = HashSet::new();
    let mut in_stack = HashSet::new();

    fn visit(
        name: &str,
        recipes: &HashMap<String, Recipe>,
        order: &mut Vec<String>,
        visited: &mut HashSet<String>,
        in_stack: &mut HashSet<String>,
    ) -> Result<()> {
        if visited.contains(name) {
            return Ok(());
        }
        if in_stack.contains(name) {
            bail!("circular dependency detected involving recipe {name:?}");
        }
        in_stack.insert(name.to_string());

        if let Some(recipe) = recipes.get(name) {
            for dep in &recipe.depends_on {
                if !recipes.contains_key(dep.as_str()) {
                    bail!(
                        "recipe {name:?} depends on {dep:?}, which does not exist in {RECIPE_FILE}",
                        RECIPE_FILE = "taskpilot.toml"
                    );
                }
                visit(dep, recipes, order, visited, in_stack)?;
            }
        }

        in_stack.remove(name);
        visited.insert(name.to_string());
        order.push(name.to_string());
        Ok(())
    }

    visit(target, &recipes, &mut order, &mut visited, &mut in_stack)?;
    Ok(order)
}

/// Check and resolve skill dependencies for a recipe.
/// Local deps (bare names) are verified. Remote deps (with /) are auto-installed if missing.
pub(crate) fn resolve_skill_deps(deps: &[String], extra_dirs: &[String]) -> Result<()> {
    let stdin = io::stdin();
    let mut reader = stdin.lock();
    resolve_skill_deps_with_reader(deps, extra_dirs, &mut reader)
}

/// Inner implementation that accepts an arbitrary reader for testability.
pub(crate) fn resolve_skill_deps_with_reader(
    deps: &[String],
    extra_dirs: &[String],
    reader: &mut dyn BufRead,
) -> Result<()> {
    if deps.is_empty() {
        return Ok(());
    }

    let skills = skill::discover(extra_dirs).context("discover skills for dep check")?;

    for dep in deps {
        let is_remote = dep.contains('/');
        let skill_name = if is_remote {
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
            reader.read_line(&mut input)?;
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
            bail!(
                "recipe requires skill {skill_name:?} which is not installed.\n  \
                 Search: taskpilot skills find {skill_name}\n  \
                 Add:    taskpilot skills add <owner/repo/{skill_name}>"
            );
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serial_test::serial;
    use tempfile::TempDir;

    /// Helper: create a TempDir, write a taskpilot.toml with the given content,
    /// and chdir into it. Returns the TempDir (must be kept alive).
    fn setup_toml(content: &str) -> TempDir {
        let dir = TempDir::new().unwrap();
        fs::write(dir.path().join("taskpilot.toml"), content).unwrap();
        std::env::set_current_dir(dir.path()).unwrap();
        dir
    }

    /// Helper: chdir to a fresh empty TempDir.
    fn setup_empty() -> TempDir {
        let dir = TempDir::new().unwrap();
        std::env::set_current_dir(dir.path()).unwrap();
        dir
    }

    // ---- load ----

    #[test]
    #[serial]
    fn load_no_file() {
        let _dir = setup_empty();
        let err = load().unwrap_err();
        assert!(err.to_string().contains("no taskpilot.toml"));
    }

    #[test]
    #[serial]
    fn load_valid_toml() {
        let _dir = setup_toml(
            r#"
[recipes.foo]
prompt = "do stuff"
"#,
        );
        let recipes = load().unwrap();
        assert_eq!(recipes.len(), 1);
        assert!(recipes.contains_key("foo"));
        assert_eq!(recipes["foo"].prompt.as_deref(), Some("do stuff"));
    }

    #[test]
    #[serial]
    fn load_invalid_toml() {
        let _dir = setup_toml("this is not valid toml [[[");
        let err = load().unwrap_err();
        assert!(err.to_string().contains("parse taskpilot.toml"));
    }

    #[test]
    #[serial]
    fn load_empty_recipes() {
        let _dir = setup_toml("[recipes]\n");
        let recipes = load().unwrap();
        assert!(recipes.is_empty());
    }

    // ---- get ----

    #[test]
    #[serial]
    fn get_exists() {
        let _dir = setup_toml(
            r#"
[recipes.alpha]
prompt = "hello"
"#,
        );
        let r = get("alpha").unwrap();
        assert_eq!(r.prompt.as_deref(), Some("hello"));
    }

    #[test]
    #[serial]
    fn get_not_found_with_available() {
        let _dir = setup_toml(
            r#"
[recipes.alpha]
prompt = "hello"
"#,
        );
        let err = get("missing").unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains("missing"));
        assert!(msg.contains("alpha"));
    }

    #[test]
    #[serial]
    fn get_not_found_none_available() {
        let _dir = setup_toml("[recipes]\n");
        let err = get("anything").unwrap_err();
        assert!(err.to_string().contains("(none)"));
    }

    // ---- list ----

    #[test]
    #[serial]
    fn list_empty() {
        let _dir = setup_toml("[recipes]\n");
        list().unwrap();
    }

    #[test]
    #[serial]
    fn list_with_prompt() {
        let _dir = setup_toml(
            r#"
[recipes.a]
prompt = "short prompt"
"#,
        );
        list().unwrap();
    }

    #[test]
    #[serial]
    fn list_with_prompt_file() {
        let _dir = setup_toml(
            r#"
[recipes.a]
prompt_file = "my_prompt.md"
"#,
        );
        list().unwrap();
    }

    #[test]
    #[serial]
    fn list_no_prompt() {
        let _dir = setup_toml(
            r#"
[recipes.a]
description = "desc"
"#,
        );
        list().unwrap();
    }

    #[test]
    #[serial]
    fn list_long_prompt_truncated() {
        let long = "a".repeat(100);
        let _dir = setup_toml(&format!(
            r#"
[recipes.a]
prompt = "{long}"
"#,
        ));
        list().unwrap();
    }

    #[test]
    #[serial]
    fn list_with_details() {
        let _dir = setup_toml(
            r#"
[recipes.a]
prompt = "do it"
input = ["file1.txt", "file2.txt"]
output_dir = "out/"
skill_deps = ["some-skill"]
description = "a recipe"
"#,
        );
        list().unwrap();
    }

    // ---- init ----

    #[test]
    #[serial]
    fn init_creates_file() {
        let _dir = setup_empty();
        init().unwrap();
        assert!(Path::new("taskpilot.toml").exists());
    }

    #[test]
    #[serial]
    fn init_already_exists() {
        let _dir = setup_toml("[recipes]\n");
        let err = init().unwrap_err();
        assert!(err.to_string().contains("already exists"));
    }

    // ---- doctor ----

    #[test]
    #[serial]
    fn doctor_happy_path() {
        let dir = TempDir::new().unwrap();
        std::env::set_current_dir(dir.path()).unwrap();
        fs::write(dir.path().join("input.txt"), "data").unwrap();
        fs::write(dir.path().join("prompt.md"), "do stuff").unwrap();
        fs::write(
            dir.path().join("taskpilot.toml"),
            r#"
[recipes.r1]
prompt_file = "prompt.md"
input = ["input.txt"]
output_dir = "out/"
"#,
        )
        .unwrap();
        std::env::set_var("ANTHROPIC_API_KEY", "sk-test");
        doctor().unwrap();
        std::env::remove_var("ANTHROPIC_API_KEY");
    }

    #[test]
    #[serial]
    fn doctor_no_toml() {
        let _dir = setup_empty();
        std::env::set_var("ANTHROPIC_API_KEY", "sk-test");
        doctor().unwrap();
        std::env::remove_var("ANTHROPIC_API_KEY");
    }

    #[test]
    #[serial]
    fn doctor_invalid_toml() {
        let _dir = setup_toml("not valid [[[");
        std::env::set_var("ANTHROPIC_API_KEY", "sk-test");
        doctor().unwrap();
        std::env::remove_var("ANTHROPIC_API_KEY");
    }

    #[test]
    #[serial]
    fn doctor_no_recipes() {
        let _dir = setup_toml("[recipes]\n");
        std::env::set_var("ANTHROPIC_API_KEY", "sk-test");
        doctor().unwrap();
        std::env::remove_var("ANTHROPIC_API_KEY");
    }

    #[test]
    #[serial]
    fn doctor_no_prompt_or_prompt_file() {
        let _dir = setup_toml(
            r#"
[recipes.bad]
input = []
"#,
        );
        std::env::set_var("ANTHROPIC_API_KEY", "sk-test");
        doctor().unwrap();
        std::env::remove_var("ANTHROPIC_API_KEY");
    }

    #[test]
    #[serial]
    fn doctor_prompt_file_missing() {
        let _dir = setup_toml(
            r#"
[recipes.r]
prompt_file = "nonexistent.md"
"#,
        );
        std::env::set_var("ANTHROPIC_API_KEY", "sk-test");
        doctor().unwrap();
        std::env::remove_var("ANTHROPIC_API_KEY");
    }

    #[test]
    #[serial]
    fn doctor_inline_prompt() {
        let _dir = setup_toml(
            r#"
[recipes.r]
prompt = "do it"
output_dir = "out/"
"#,
        );
        std::env::set_var("ANTHROPIC_API_KEY", "sk-test");
        doctor().unwrap();
        std::env::remove_var("ANTHROPIC_API_KEY");
    }

    #[test]
    #[serial]
    fn doctor_input_missing() {
        let _dir = setup_toml(
            r#"
[recipes.r]
prompt = "go"
input = ["nope.txt"]
"#,
        );
        std::env::set_var("ANTHROPIC_API_KEY", "sk-test");
        doctor().unwrap();
        std::env::remove_var("ANTHROPIC_API_KEY");
    }

    #[test]
    #[serial]
    fn doctor_input_exists() {
        let dir = TempDir::new().unwrap();
        std::env::set_current_dir(dir.path()).unwrap();
        fs::write(dir.path().join("data.txt"), "").unwrap();
        fs::write(
            dir.path().join("taskpilot.toml"),
            r#"
[recipes.r]
prompt = "go"
input = ["data.txt"]
output_dir = "out/"
"#,
        )
        .unwrap();
        std::env::set_var("ANTHROPIC_API_KEY", "sk-test");
        doctor().unwrap();
        std::env::remove_var("ANTHROPIC_API_KEY");
    }

    #[test]
    #[serial]
    fn doctor_no_output_dir_warning() {
        let _dir = setup_toml(
            r#"
[recipes.r]
prompt = "go"
"#,
        );
        std::env::set_var("ANTHROPIC_API_KEY", "sk-test");
        doctor().unwrap();
        std::env::remove_var("ANTHROPIC_API_KEY");
    }

    #[test]
    #[serial]
    fn doctor_skill_dep_local_not_installed() {
        let _dir = setup_toml(
            r#"
[recipes.r]
prompt = "go"
output_dir = "out/"
skill_deps = ["nonexistent-skill"]
"#,
        );
        std::env::set_var("ANTHROPIC_API_KEY", "sk-test");
        doctor().unwrap();
        std::env::remove_var("ANTHROPIC_API_KEY");
    }

    #[test]
    #[serial]
    fn doctor_skill_dep_remote_not_installed() {
        let _dir = setup_toml(
            r#"
[recipes.r]
prompt = "go"
output_dir = "out/"
skill_deps = ["owner/repo/remote-skill"]
"#,
        );
        std::env::set_var("ANTHROPIC_API_KEY", "sk-test");
        doctor().unwrap();
        std::env::remove_var("ANTHROPIC_API_KEY");
    }

    #[test]
    #[serial]
    fn doctor_depends_on_valid() {
        let _dir = setup_toml(
            r#"
[recipes.a]
prompt = "go"
output_dir = "out/"

[recipes.b]
prompt = "then"
output_dir = "out/"
depends_on = ["a"]
"#,
        );
        std::env::set_var("ANTHROPIC_API_KEY", "sk-test");
        doctor().unwrap();
        std::env::remove_var("ANTHROPIC_API_KEY");
    }

    #[test]
    #[serial]
    fn doctor_depends_on_invalid() {
        let _dir = setup_toml(
            r#"
[recipes.a]
prompt = "go"
output_dir = "out/"
depends_on = ["nonexistent"]
"#,
        );
        std::env::set_var("ANTHROPIC_API_KEY", "sk-test");
        doctor().unwrap();
        std::env::remove_var("ANTHROPIC_API_KEY");
    }

    #[test]
    #[serial]
    fn doctor_circular_dependency() {
        let _dir = setup_toml(
            r#"
[recipes.a]
prompt = "go"
output_dir = "out/"
depends_on = ["b"]

[recipes.b]
prompt = "go"
output_dir = "out/"
depends_on = ["a"]
"#,
        );
        std::env::set_var("ANTHROPIC_API_KEY", "sk-test");
        doctor().unwrap();
        std::env::remove_var("ANTHROPIC_API_KEY");
    }

    #[test]
    #[serial]
    fn doctor_allow_bash_true() {
        let _dir = setup_toml(
            r#"
[recipes.r]
prompt = "go"
output_dir = "out/"
allow_bash = true
"#,
        );
        std::env::set_var("ANTHROPIC_API_KEY", "sk-test");
        doctor().unwrap();
        std::env::remove_var("ANTHROPIC_API_KEY");
    }

    #[test]
    #[serial]
    fn doctor_allow_bash_false() {
        let _dir = setup_toml(
            r#"
[recipes.r]
prompt = "go"
output_dir = "out/"
allow_bash = false
"#,
        );
        std::env::set_var("ANTHROPIC_API_KEY", "sk-test");
        doctor().unwrap();
        std::env::remove_var("ANTHROPIC_API_KEY");
    }

    #[test]
    #[serial]
    fn doctor_no_api_key() {
        let _dir = setup_toml(
            r#"
[recipes.r]
prompt = "go"
output_dir = "out/"
"#,
        );
        std::env::remove_var("ANTHROPIC_API_KEY");
        doctor().unwrap();
    }

    // ---- resolve_depends_on ----

    #[test]
    #[serial]
    fn resolve_single_no_deps() {
        let _dir = setup_toml(
            r#"
[recipes.solo]
prompt = "go"
"#,
        );
        let order = resolve_depends_on("solo").unwrap();
        assert_eq!(order, vec!["solo"]);
    }

    #[test]
    #[serial]
    fn resolve_linear_chain() {
        let _dir = setup_toml(
            r#"
[recipes.a]
prompt = "1"

[recipes.b]
prompt = "2"
depends_on = ["a"]

[recipes.c]
prompt = "3"
depends_on = ["b"]
"#,
        );
        let order = resolve_depends_on("c").unwrap();
        assert_eq!(order, vec!["a", "b", "c"]);
    }

    #[test]
    #[serial]
    fn resolve_target_not_found() {
        let _dir = setup_toml(
            r#"
[recipes.a]
prompt = "1"
"#,
        );
        let err = resolve_depends_on("missing").unwrap_err();
        assert!(err.to_string().contains("not found"));
    }

    #[test]
    #[serial]
    fn resolve_dep_references_nonexistent() {
        let _dir = setup_toml(
            r#"
[recipes.a]
prompt = "1"
depends_on = ["ghost"]
"#,
        );
        let err = resolve_depends_on("a").unwrap_err();
        assert!(err.to_string().contains("ghost"));
        assert!(err.to_string().contains("does not exist"));
    }

    #[test]
    #[serial]
    fn resolve_circular() {
        let _dir = setup_toml(
            r#"
[recipes.a]
prompt = "1"
depends_on = ["b"]

[recipes.b]
prompt = "2"
depends_on = ["a"]
"#,
        );
        let err = resolve_depends_on("a").unwrap_err();
        assert!(err.to_string().contains("circular"));
    }

    #[test]
    #[serial]
    fn resolve_shared_dependency() {
        let _dir = setup_toml(
            r#"
[recipes.a]
prompt = "1"

[recipes.b]
prompt = "2"
depends_on = ["a"]

[recipes.c]
prompt = "3"
depends_on = ["a", "b"]
"#,
        );
        let order = resolve_depends_on("c").unwrap();
        assert_eq!(order.iter().filter(|x| *x == "a").count(), 1);
        assert!(order.iter().position(|x| x == "a") < order.iter().position(|x| x == "b"));
        assert!(order.iter().position(|x| x == "b") < order.iter().position(|x| x == "c"));
    }

    // ---- resolve_skill_deps_with_reader ----

    #[test]
    #[serial]
    fn skill_deps_empty() {
        let _dir = setup_empty();
        let mut reader = io::Cursor::new(b"");
        resolve_skill_deps_with_reader(&[], &[], &mut reader).unwrap();
    }

    #[test]
    #[serial]
    fn skill_deps_local_not_installed() {
        let _dir = setup_empty();
        let mut reader = io::Cursor::new(b"");
        let err =
            resolve_skill_deps_with_reader(&["nonexistent".to_string()], &[], &mut reader)
                .unwrap_err();
        assert!(err.to_string().contains("nonexistent"));
        assert!(err.to_string().contains("not installed"));
    }

    #[test]
    #[serial]
    fn skill_deps_remote_cancel() {
        let _dir = setup_empty();
        let mut reader = io::Cursor::new(b"c\n");
        let err = resolve_skill_deps_with_reader(
            &["owner/repo/myskill".to_string()],
            &[],
            &mut reader,
        )
        .unwrap_err();
        assert!(err.to_string().contains("installation cancelled"));
    }

    #[test]
    #[serial]
    fn skill_deps_remote_unknown_input_cancels() {
        let _dir = setup_empty();
        let mut reader = io::Cursor::new(b"x\n");
        let err = resolve_skill_deps_with_reader(
            &["owner/repo/myskill".to_string()],
            &[],
            &mut reader,
        )
        .unwrap_err();
        assert!(err.to_string().contains("installation cancelled"));
    }

    // ---- print_summary ----

    #[test]
    fn print_summary_no_issues() {
        print_summary(0, 0);
    }

    #[test]
    fn print_summary_errors_only() {
        print_summary(3, 0);
    }

    #[test]
    fn print_summary_warnings_only() {
        print_summary(0, 2);
    }

    #[test]
    fn print_summary_both() {
        print_summary(1, 1);
    }

    #[test]
    #[serial]
    fn skill_deps_local_installed() {
        let dir = setup_empty();
        // Create a skill that can be discovered
        let skill_dir = dir.path().join(".agents").join("skills").join("myskill");
        std::fs::create_dir_all(&skill_dir).unwrap();
        std::fs::write(skill_dir.join("SKILL.md"), "# My Skill").unwrap();
        let mut reader = io::Cursor::new(b"");
        let result =
            resolve_skill_deps_with_reader(&["myskill".to_string()], &[], &mut reader);
        assert!(result.is_ok());
    }

    #[test]
    #[serial]
    fn skill_deps_remote_installed() {
        let dir = setup_empty();
        // Create a skill that matches the remote dep name
        let skill_dir = dir.path().join(".agents").join("skills").join("myskill");
        std::fs::create_dir_all(&skill_dir).unwrap();
        std::fs::write(skill_dir.join("SKILL.md"), "# My Skill").unwrap();
        let mut reader = io::Cursor::new(b"");
        let result = resolve_skill_deps_with_reader(
            &["owner/repo/myskill".to_string()],
            &[],
            &mut reader,
        );
        assert!(result.is_ok());
    }

    #[test]
    #[serial]
    fn doctor_skill_dep_installed() {
        let dir = setup_empty();
        // Create a skill and a recipe that depends on it
        let skill_dir = dir.path().join(".agents").join("skills").join("myskill");
        std::fs::create_dir_all(&skill_dir).unwrap();
        std::fs::write(skill_dir.join("SKILL.md"), "# My Skill").unwrap();
        std::fs::write(
            dir.path().join("taskpilot.toml"),
            r#"
[recipes.test]
prompt = "go"
skill_deps = ["myskill"]
"#,
        )
        .unwrap();
        std::env::set_var("ANTHROPIC_API_KEY", "test");
        doctor().unwrap();
        std::env::remove_var("ANTHROPIC_API_KEY");
    }
}
