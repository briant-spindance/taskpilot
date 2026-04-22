use anyhow::{Context, Result};
use serde::Deserialize;
use std::collections::HashMap;
use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use walkdir::WalkDir;

#[derive(Debug, Clone)]
pub struct Skill {
    pub name: String,
    pub description: String,
    pub path: PathBuf,
}

#[derive(Debug, Deserialize, Default)]
struct Frontmatter {
    #[serde(default)]
    name: String,
    #[serde(default)]
    description: String,
}

/// Discover all skills from the standard search directories.
pub fn discover() -> Result<Vec<Skill>> {
    let home = dirs_path()?;
    let cwd = env::current_dir().context("resolve working dir")?;

    let mut search_dirs = vec![
        cwd.join(".agents").join("skills"),
        cwd.join(".taskpilot").join("skills"),
        home.join(".agents").join("skills"),
    ];

    if let Ok(override_dir) = env::var("TASKPILOT_SKILLS_DIR") {
        search_dirs.push(PathBuf::from(override_dir));
    } else {
        search_dirs.push(home.join(".taskpilot").join("skills"));
    }

    let mut seen = HashMap::new();
    let mut skills = Vec::new();

    for dir in &search_dirs {
        let entries = match fs::read_dir(dir) {
            Ok(e) => e,
            Err(_) => continue,
        };
        for entry in entries.flatten() {
            if !entry.file_type().map(|t| t.is_dir()).unwrap_or(false) {
                continue;
            }
            let name = entry.file_name().to_string_lossy().to_string();
            if seen.contains_key(&name) {
                continue;
            }
            let skill_dir = entry.path();
            let skill_file = skill_dir.join("SKILL.md");
            if !skill_file.exists() {
                continue;
            }
            if let Ok(s) = parse(&skill_dir) {
                seen.insert(name, true);
                skills.push(s);
            }
        }
    }

    Ok(skills)
}

/// Parse a SKILL.md from the given directory.
pub fn parse(skill_dir: &Path) -> Result<Skill> {
    let skill_file = skill_dir.join("SKILL.md");
    let content = fs::read_to_string(&skill_file).context("read SKILL.md")?;
    let fallback_name = skill_dir
        .file_name()
        .unwrap_or_default()
        .to_string_lossy()
        .to_string();

    let mut fm = Frontmatter::default();
    if content.starts_with("---\n") {
        if let Some(end) = content[4..].find("\n---") {
            let yaml_str = &content[4..4 + end];
            if let Ok(parsed) = serde_yaml::from_str::<Frontmatter>(yaml_str) {
                fm = parsed;
            }
        }
    }

    let name = if fm.name.is_empty() {
        fallback_name
    } else {
        fm.name
    };

    Ok(Skill {
        name,
        description: fm.description,
        path: skill_dir.to_path_buf(),
    })
}

/// Build catalog entries (name + description) for the system prompt.
pub fn build_catalog(skills: &[Skill]) -> Vec<(String, String)> {
    skills
        .iter()
        .map(|s| (s.name.clone(), s.description.clone()))
        .collect()
}

/// Find a skill by name.
pub fn find_by_name<'a>(skills: &'a [Skill], name: &str) -> Result<&'a Skill> {
    skills
        .iter()
        .find(|s| s.name == name)
        .with_context(|| format!("skill {name:?} not found"))
}

/// Activate a skill: read full SKILL.md and enumerate resources.
pub fn activate(skill: &Skill) -> Result<String> {
    let content = fs::read_to_string(skill.path.join("SKILL.md")).context("read SKILL.md")?;
    let resources = enumerate_resources(&skill.path);

    let mut out = format!("<skill_content name={:?}>\n{}\n", skill.name, content);
    if !resources.is_empty() {
        out.push_str("<skill_resources>\n");
        for r in &resources {
            out.push_str(&format!("  {r}\n"));
        }
        out.push_str("</skill_resources>\n");
    }
    out.push_str("</skill_content>\n");
    Ok(out)
}

fn enumerate_resources(skill_dir: &Path) -> Vec<String> {
    let mut resources = Vec::new();
    for subdir in &["scripts", "references", "assets"] {
        let base = skill_dir.join(subdir);
        if !base.exists() {
            continue;
        }
        for entry in WalkDir::new(&base).into_iter().flatten() {
            if entry.file_type().is_file() {
                if let Ok(rel) = entry.path().strip_prefix(skill_dir) {
                    resources.push(rel.to_string_lossy().to_string());
                }
            }
        }
    }
    resources
}

fn dirs_path() -> Result<PathBuf> {
    env::var("HOME")
        .map(PathBuf::from)
        .context("resolve home directory")
}
