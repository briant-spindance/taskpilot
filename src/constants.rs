/// Default Anthropic model used when none is specified.
pub(crate) const DEFAULT_MODEL: &str = "claude-sonnet-4-20250514";

/// Filename for skill definitions.
pub(crate) const SKILL_FILE: &str = "SKILL.md";

/// Top-level directory name for agent-related files.
pub(crate) const AGENTS_DIR: &str = ".agents";

/// Subdirectory name for installed skills.
pub(crate) const SKILLS_DIR: &str = "skills";

/// Resource subdirectories scanned inside a skill.
pub(crate) const RESOURCE_DIRS: &[&str] = &["scripts", "references", "assets"];

/// Resolve the user's home directory from `$HOME`.
pub(crate) fn home_dir() -> anyhow::Result<std::path::PathBuf> {
    std::env::var("HOME")
        .map(std::path::PathBuf::from)
        .map_err(|_| anyhow::anyhow!("resolve home directory"))
}
