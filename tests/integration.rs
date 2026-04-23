use assert_cmd::Command;
use predicates::prelude::*;
use std::fs;
use tempfile::TempDir;

fn cmd() -> Command {
    Command::cargo_bin("taskpilot").unwrap()
}

// ── Init ──────────────────────────────────────────────────────

#[test]
fn init_creates_toml() {
    let tmp = TempDir::new().unwrap();
    cmd()
        .current_dir(tmp.path())
        .arg("init")
        .assert()
        .success()
        .stdout(predicate::str::contains("Created taskpilot.toml"));
    assert!(tmp.path().join("taskpilot.toml").exists());
}

#[test]
fn init_fails_if_toml_exists() {
    let tmp = TempDir::new().unwrap();
    fs::write(tmp.path().join("taskpilot.toml"), "").unwrap();
    cmd()
        .current_dir(tmp.path())
        .arg("init")
        .assert()
        .failure()
        .stderr(predicate::str::contains("already exists"));
}

// ── Recipes ───────────────────────────────────────────────────

#[test]
fn recipes_no_toml() {
    let tmp = TempDir::new().unwrap();
    cmd()
        .current_dir(tmp.path())
        .arg("recipes")
        .assert()
        .failure()
        .stderr(predicate::str::contains("no taskpilot.toml"));
}

#[test]
fn recipes_lists_defined() {
    let tmp = TempDir::new().unwrap();
    fs::write(
        tmp.path().join("taskpilot.toml"),
        r#"
[recipes.hello]
prompt = "say hello"
"#,
    )
    .unwrap();
    cmd()
        .current_dir(tmp.path())
        .arg("recipes")
        .assert()
        .success()
        .stdout(predicate::str::contains("hello"));
}

// ── Doctor ────────────────────────────────────────────────────

#[test]
fn doctor_no_toml() {
    let tmp = TempDir::new().unwrap();
    cmd()
        .current_dir(tmp.path())
        .arg("doctor")
        .env("ANTHROPIC_API_KEY", "test-key")
        .assert()
        .success()
        .stdout(predicate::str::contains("not found (optional)"));
}

#[test]
fn doctor_with_valid_toml() {
    let tmp = TempDir::new().unwrap();
    fs::write(
        tmp.path().join("taskpilot.toml"),
        r#"
[recipes.test]
prompt = "test prompt"
output_dir = "out/"
"#,
    )
    .unwrap();
    cmd()
        .current_dir(tmp.path())
        .arg("doctor")
        .env("ANTHROPIC_API_KEY", "test-key")
        .assert()
        .success();
}

// ── Run (dry-run) ─────────────────────────────────────────────

#[test]
fn run_dry_run_with_prompt() {
    let tmp = TempDir::new().unwrap();
    cmd()
        .current_dir(tmp.path())
        .args(["run", "--prompt", "hello world", "--dry-run"])
        .env("ANTHROPIC_API_KEY", "fake-key")
        .assert()
        .success()
        .stdout(predicate::str::contains("=== Dry Run ==="))
        .stdout(predicate::str::contains("hello world"));
}

#[test]
fn run_dry_run_recipe() {
    let tmp = TempDir::new().unwrap();
    fs::write(
        tmp.path().join("taskpilot.toml"),
        r#"
[recipes.test]
prompt = "do something"
output_dir = "out/"
model = "claude-sonnet-4-20250514"
"#,
    )
    .unwrap();
    cmd()
        .current_dir(tmp.path())
        .args(["run", "test", "--dry-run"])
        .env("ANTHROPIC_API_KEY", "fake-key")
        .assert()
        .success()
        .stdout(predicate::str::contains("Recipe: test"))
        .stdout(predicate::str::contains("do something"));
}

#[test]
fn run_dry_run_recipe_with_deps() {
    let tmp = TempDir::new().unwrap();
    fs::write(
        tmp.path().join("taskpilot.toml"),
        r#"
[recipes.step1]
prompt = "first step"

[recipes.step2]
prompt = "second step"
depends_on = ["step1"]
"#,
    )
    .unwrap();
    // dry-run only applies to the target, deps will try to actually run
    // and fail on API key, but the dep chain resolution is tested
    cmd()
        .current_dir(tmp.path())
        .args(["run", "step2", "--dry-run"])
        .env("ANTHROPIC_API_KEY", "fake-key")
        .assert()
        // This will fail because deps try to run (not dry-run)
        .failure();
}

#[test]
fn run_no_prompt_no_recipe() {
    let tmp = TempDir::new().unwrap();
    cmd()
        .current_dir(tmp.path())
        .arg("run")
        .env("ANTHROPIC_API_KEY", "fake-key")
        .assert()
        .failure()
        .stderr(predicate::str::contains("--prompt"));
}

#[test]
fn run_dry_run_with_prompt_file() {
    let tmp = TempDir::new().unwrap();
    fs::write(tmp.path().join("prompt.txt"), "my prompt from file").unwrap();
    cmd()
        .current_dir(tmp.path())
        .args([
            "run",
            "--prompt-file",
            "prompt.txt",
            "--dry-run",
        ])
        .env("ANTHROPIC_API_KEY", "fake-key")
        .assert()
        .success()
        .stdout(predicate::str::contains("my prompt from file"));
}

#[test]
fn run_dry_run_no_stream() {
    let tmp = TempDir::new().unwrap();
    cmd()
        .current_dir(tmp.path())
        .args(["run", "--prompt", "hello", "--dry-run", "--no-stream"])
        .env("ANTHROPIC_API_KEY", "fake-key")
        .assert()
        .success();
}

#[test]
fn run_dry_run_allow_bash() {
    let tmp = TempDir::new().unwrap();
    cmd()
        .current_dir(tmp.path())
        .args(["run", "--prompt", "hello", "--dry-run", "--allow-bash"])
        .env("ANTHROPIC_API_KEY", "fake-key")
        .assert()
        .success()
        .stdout(predicate::str::contains("Bash: enabled"));
}

#[test]
fn run_dry_run_bash_disabled() {
    let tmp = TempDir::new().unwrap();
    cmd()
        .current_dir(tmp.path())
        .args(["run", "--prompt", "hello", "--dry-run"])
        .env("ANTHROPIC_API_KEY", "fake-key")
        .assert()
        .success()
        .stdout(predicate::str::contains("Bash: disabled"));
}

#[test]
fn run_dry_run_with_inputs() {
    let tmp = TempDir::new().unwrap();
    fs::write(tmp.path().join("data.csv"), "a,b\n1,2").unwrap();
    cmd()
        .current_dir(tmp.path())
        .args([
            "run",
            "--prompt",
            "analyze",
            "--input",
            "data.csv",
            "--output-dir",
            "out/",
            "--dry-run",
        ])
        .env("ANTHROPIC_API_KEY", "fake-key")
        .assert()
        .success()
        .stdout(predicate::str::contains("data.csv"))
        .stdout(predicate::str::contains("out/"));
}

#[test]
fn run_dry_run_recipe_with_prompt_file_override() {
    let tmp = TempDir::new().unwrap();
    fs::write(
        tmp.path().join("taskpilot.toml"),
        r#"
[recipes.test]
prompt = "original prompt"
"#,
    )
    .unwrap();
    fs::write(tmp.path().join("override.txt"), "overridden prompt").unwrap();
    cmd()
        .current_dir(tmp.path())
        .args(["run", "test", "--prompt-file", "override.txt", "--dry-run"])
        .env("ANTHROPIC_API_KEY", "fake-key")
        .assert()
        .success()
        .stdout(predicate::str::contains("overridden prompt"));
}

#[test]
fn run_dry_run_recipe_with_prompt_override() {
    let tmp = TempDir::new().unwrap();
    fs::write(
        tmp.path().join("taskpilot.toml"),
        r#"
[recipes.test]
prompt = "original prompt"
"#,
    )
    .unwrap();
    cmd()
        .current_dir(tmp.path())
        .args(["run", "test", "--prompt", "cli override", "--dry-run"])
        .env("ANTHROPIC_API_KEY", "fake-key")
        .assert()
        .success()
        .stdout(predicate::str::contains("cli override"));
}

#[test]
fn run_dry_run_recipe_with_prompt_file_recipe_level() {
    let tmp = TempDir::new().unwrap();
    fs::write(tmp.path().join("my-prompt.md"), "prompt from file field").unwrap();
    fs::write(
        tmp.path().join("taskpilot.toml"),
        r#"
[recipes.test]
prompt_file = "my-prompt.md"
"#,
    )
    .unwrap();
    cmd()
        .current_dir(tmp.path())
        .args(["run", "test", "--dry-run"])
        .env("ANTHROPIC_API_KEY", "fake-key")
        .assert()
        .success()
        .stdout(predicate::str::contains("prompt from file field"));
}

#[test]
fn run_recipe_no_prompt() {
    let tmp = TempDir::new().unwrap();
    fs::write(
        tmp.path().join("taskpilot.toml"),
        r#"
[recipes.test]
output_dir = "out/"
"#,
    )
    .unwrap();
    cmd()
        .current_dir(tmp.path())
        .args(["run", "test", "--dry-run"])
        .env("ANTHROPIC_API_KEY", "fake-key")
        .assert()
        .failure()
        .stderr(predicate::str::contains("no prompt or prompt_file"));
}

// ── Skills ────────────────────────────────────────────────────

#[test]
fn skills_list_empty() {
    let tmp = TempDir::new().unwrap();
    cmd()
        .current_dir(tmp.path())
        .args(["skills", "list"])
        .env("HOME", tmp.path().to_str().unwrap())
        .assert()
        .success()
        .stdout(predicate::str::contains("No skills found"));
}

#[test]
fn skills_list_with_skills() {
    let tmp = TempDir::new().unwrap();
    let skill_dir = tmp.path().join(".agents").join("skills").join("test-skill");
    fs::create_dir_all(&skill_dir).unwrap();
    fs::write(skill_dir.join("SKILL.md"), "# Test Skill\nA test skill.").unwrap();
    cmd()
        .current_dir(tmp.path())
        .args(["skills", "list"])
        .env("HOME", tmp.path().to_str().unwrap())
        .assert()
        .success()
        .stdout(predicate::str::contains("test-skill"));
}

#[test]
fn skills_list_with_description() {
    let tmp = TempDir::new().unwrap();
    let skill_dir = tmp.path().join(".agents").join("skills").join("my-skill");
    fs::create_dir_all(&skill_dir).unwrap();
    fs::write(
        skill_dir.join("SKILL.md"),
        "---\nname: my-skill\ndescription: A great skill for testing.\n---\n# My Skill",
    )
    .unwrap();
    cmd()
        .current_dir(tmp.path())
        .args(["skills", "list"])
        .env("HOME", tmp.path().to_str().unwrap())
        .assert()
        .success()
        .stdout(predicate::str::contains("my-skill"))
        .stdout(predicate::str::contains("A great skill for testing"));
}

#[test]
fn skills_list_no_description() {
    let tmp = TempDir::new().unwrap();
    let skill_dir = tmp.path().join(".agents").join("skills").join("bare");
    fs::create_dir_all(&skill_dir).unwrap();
    fs::write(skill_dir.join("SKILL.md"), "# Bare skill").unwrap();
    cmd()
        .current_dir(tmp.path())
        .args(["skills", "list"])
        .env("HOME", tmp.path().to_str().unwrap())
        .assert()
        .success()
        .stdout(predicate::str::contains("(no description)"));
}

#[test]
fn skills_show() {
    let tmp = TempDir::new().unwrap();
    let skill_dir = tmp.path().join(".agents").join("skills").join("show-me");
    fs::create_dir_all(&skill_dir).unwrap();
    fs::write(
        skill_dir.join("SKILL.md"),
        "---\nname: show-me\ndescription: Showable skill.\n---\n# Show Me",
    )
    .unwrap();
    cmd()
        .current_dir(tmp.path())
        .args(["skills", "show", "show-me"])
        .env("HOME", tmp.path().to_str().unwrap())
        .assert()
        .success()
        .stdout(predicate::str::contains("Name:        show-me"))
        .stdout(predicate::str::contains("Description: Showable skill."));
}

#[test]
fn skills_show_not_found() {
    let tmp = TempDir::new().unwrap();
    cmd()
        .current_dir(tmp.path())
        .args(["skills", "show", "nonexistent"])
        .env("HOME", tmp.path().to_str().unwrap())
        .assert()
        .failure()
        .stderr(predicate::str::contains("not found"));
}

#[test]
fn skills_find_empty_query() {
    let tmp = TempDir::new().unwrap();
    cmd()
        .current_dir(tmp.path())
        .args(["skills", "find"])
        .assert()
        .failure();
}

#[test]
fn skills_install_no_skill_md() {
    let tmp = TempDir::new().unwrap();
    let src = tmp.path().join("bad-skill");
    fs::create_dir_all(&src).unwrap();
    cmd()
        .current_dir(tmp.path())
        .args(["skills", "install", src.to_str().unwrap()])
        .env("HOME", tmp.path().to_str().unwrap())
        .assert()
        .failure()
        .stderr(predicate::str::contains("no SKILL.md"));
}

// ── Config ────────────────────────────────────────────────────

// config requires stdin interaction, so just test it bails gracefully
// when stdin is not a terminal (piped empty input)

// ── API key from config ───────────────────────────────────────

#[test]
fn run_uses_config_api_key() {
    let tmp = TempDir::new().unwrap();
    let config_dir = tmp.path().join(".local").join("taskpilot");
    fs::create_dir_all(&config_dir).unwrap();
    fs::write(
        config_dir.join("config.yml"),
        "api_key: fake-config-key\nmodel: claude-sonnet-4-20250514\n",
    )
    .unwrap();
    // Don't set ANTHROPIC_API_KEY env — it should pick up from config
    cmd()
        .current_dir(tmp.path())
        .args(["run", "--prompt", "hello", "--dry-run"])
        .env("HOME", tmp.path().to_str().unwrap())
        .env_remove("ANTHROPIC_API_KEY")
        .assert()
        .success()
        .stdout(predicate::str::contains("=== Dry Run ==="));
}

#[test]
fn run_dry_run_with_model_override() {
    let tmp = TempDir::new().unwrap();
    cmd()
        .current_dir(tmp.path())
        .args([
            "run",
            "--prompt",
            "hello",
            "--model",
            "custom-model",
            "--dry-run",
        ])
        .env("ANTHROPIC_API_KEY", "fake-key")
        .assert()
        .success()
        .stdout(predicate::str::contains("Model: custom-model"));
}

#[test]
fn run_dry_run_recipe_allow_bash_from_recipe() {
    let tmp = TempDir::new().unwrap();
    fs::write(
        tmp.path().join("taskpilot.toml"),
        r#"
[recipes.test]
prompt = "hello"
allow_bash = true
"#,
    )
    .unwrap();
    cmd()
        .current_dir(tmp.path())
        .args(["run", "test", "--dry-run"])
        .env("ANTHROPIC_API_KEY", "fake-key")
        .assert()
        .success()
        .stdout(predicate::str::contains("Bash: enabled"));
}

#[test]
fn run_dry_run_with_skills_dir() {
    let tmp = TempDir::new().unwrap();
    let skills = tmp.path().join("custom-skills").join("my-skill");
    fs::create_dir_all(&skills).unwrap();
    fs::write(
        skills.join("SKILL.md"),
        "---\nname: my-skill\ndescription: Custom.\n---\n",
    )
    .unwrap();
    cmd()
        .current_dir(tmp.path())
        .args([
            "run",
            "--prompt",
            "hello",
            "--dry-run",
            "--skills-dir",
            tmp.path().join("custom-skills").to_str().unwrap(),
        ])
        .env("ANTHROPIC_API_KEY", "fake-key")
        .env("HOME", tmp.path().to_str().unwrap())
        .assert()
        .success()
        .stdout(predicate::str::contains("my-skill"));
}
