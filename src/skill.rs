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
/// Extra directories (e.g. from --skills-dir flags) are appended to the search path.
pub fn discover(extra_dirs: &[String]) -> Result<Vec<Skill>> {
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

    for dir in extra_dirs {
        search_dirs.push(PathBuf::from(dir));
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

pub(crate) fn enumerate_resources(skill_dir: &Path) -> Vec<String> {
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

pub(crate) fn dirs_path() -> Result<PathBuf> {
    env::var("HOME")
        .map(PathBuf::from)
        .context("resolve home directory")
}

#[cfg(test)]
mod tests {
    use super::*;
    use serial_test::serial;
    use std::fs;
    use tempfile::TempDir;

    fn make_skill(dir: &Path, content: &str) {
        fs::create_dir_all(dir).unwrap();
        fs::write(dir.join("SKILL.md"), content).unwrap();
    }

    // ── parse ──────────────────────────────────────────────

    #[test]
    fn parse_valid_frontmatter() {
        let tmp = TempDir::new().unwrap();
        let skill_dir = tmp.path().join("my-skill");
        make_skill(
            &skill_dir,
            "---\nname: cool\ndescription: does stuff\n---\nbody\n",
        );
        let s = parse(&skill_dir).unwrap();
        assert_eq!(s.name, "cool");
        assert_eq!(s.description, "does stuff");
    }

    #[test]
    fn parse_empty_name_falls_back_to_dir() {
        let tmp = TempDir::new().unwrap();
        let skill_dir = tmp.path().join("fallback-dir");
        make_skill(
            &skill_dir,
            "---\nname: \"\"\ndescription: hi\n---\nbody\n",
        );
        let s = parse(&skill_dir).unwrap();
        assert_eq!(s.name, "fallback-dir");
        assert_eq!(s.description, "hi");
    }

    #[test]
    fn parse_no_frontmatter() {
        let tmp = TempDir::new().unwrap();
        let skill_dir = tmp.path().join("plain");
        make_skill(&skill_dir, "Just some markdown\n");
        let s = parse(&skill_dir).unwrap();
        assert_eq!(s.name, "plain");
        assert_eq!(s.description, "");
    }

    #[test]
    fn parse_invalid_yaml() {
        let tmp = TempDir::new().unwrap();
        let skill_dir = tmp.path().join("bad-yaml");
        make_skill(&skill_dir, "---\n: : :\n---\nbody\n");
        let s = parse(&skill_dir).unwrap();
        assert_eq!(s.name, "bad-yaml");
        assert_eq!(s.description, "");
    }

    #[test]
    fn parse_no_closing_fence() {
        let tmp = TempDir::new().unwrap();
        let skill_dir = tmp.path().join("no-close");
        make_skill(&skill_dir, "---\nname: x\ndescription: y\nbody\n");
        let s = parse(&skill_dir).unwrap();
        assert_eq!(s.name, "no-close");
        assert_eq!(s.description, "");
    }

    #[test]
    fn parse_missing_file() {
        let tmp = TempDir::new().unwrap();
        let skill_dir = tmp.path().join("ghost");
        fs::create_dir_all(&skill_dir).unwrap();
        assert!(parse(&skill_dir).is_err());
    }

    // ── discover ───────────────────────────────────────────

    #[test]
    #[serial]
    fn discover_empty_search_dirs() {
        let tmp = TempDir::new().unwrap();
        env::set_var("HOME", tmp.path());
        env::remove_var("TASKPILOT_SKILLS_DIR");
        // CWD pointing to an empty dir means no .agents/skills etc.
        env::set_current_dir(tmp.path()).unwrap();
        let skills = discover(&[]).unwrap();
        assert!(skills.is_empty());
    }

    #[test]
    #[serial]
    fn discover_skills_in_agents_dir() {
        let tmp = TempDir::new().unwrap();
        let cwd = tmp.path().join("project");
        let skill_dir = cwd.join(".agents").join("skills").join("alpha");
        make_skill(
            &skill_dir,
            "---\nname: alpha\ndescription: a skill\n---\n",
        );
        env::set_var("HOME", tmp.path());
        env::remove_var("TASKPILOT_SKILLS_DIR");
        env::set_current_dir(&cwd).unwrap();
        let skills = discover(&[]).unwrap();
        assert_eq!(skills.len(), 1);
        assert_eq!(skills[0].name, "alpha");
    }

    #[test]
    #[serial]
    fn discover_duplicate_names_first_wins() {
        let tmp = TempDir::new().unwrap();
        let cwd = tmp.path().join("proj");

        // First in .agents/skills
        let s1 = cwd.join(".agents").join("skills").join("dup");
        make_skill(&s1, "---\nname: dup\ndescription: first\n---\n");

        // Second in .taskpilot/skills
        let s2 = cwd.join(".taskpilot").join("skills").join("dup");
        make_skill(&s2, "---\nname: dup\ndescription: second\n---\n");

        env::set_var("HOME", tmp.path());
        env::remove_var("TASKPILOT_SKILLS_DIR");
        env::set_current_dir(&cwd).unwrap();

        let skills = discover(&[]).unwrap();
        let dup: Vec<_> = skills.iter().filter(|s| s.name == "dup").collect();
        assert_eq!(dup.len(), 1);
        assert_eq!(dup[0].description, "first");
    }

    #[test]
    #[serial]
    fn discover_skips_non_dirs_and_missing_skill_md() {
        let tmp = TempDir::new().unwrap();
        let cwd = tmp.path().join("proj");
        let base = cwd.join(".agents").join("skills");
        fs::create_dir_all(&base).unwrap();

        // A file, not a dir
        fs::write(base.join("not-a-dir"), "hi").unwrap();

        // A dir without SKILL.md
        fs::create_dir_all(base.join("empty-dir")).unwrap();

        env::set_var("HOME", tmp.path());
        env::remove_var("TASKPILOT_SKILLS_DIR");
        env::set_current_dir(&cwd).unwrap();

        let skills = discover(&[]).unwrap();
        assert!(skills.is_empty());
    }

    #[test]
    #[serial]
    fn discover_env_var_override() {
        let tmp = TempDir::new().unwrap();
        let cwd = tmp.path().join("proj");
        fs::create_dir_all(&cwd).unwrap();

        let override_base = tmp.path().join("custom-skills");
        let skill_dir = override_base.join("beta");
        make_skill(&skill_dir, "---\nname: beta\ndescription: env\n---\n");

        env::set_var("HOME", tmp.path());
        env::set_var("TASKPILOT_SKILLS_DIR", override_base.to_str().unwrap());
        env::set_current_dir(&cwd).unwrap();

        let skills = discover(&[]).unwrap();
        assert!(skills.iter().any(|s| s.name == "beta"));
    }

    #[test]
    #[serial]
    fn discover_extra_dirs() {
        let tmp = TempDir::new().unwrap();
        let cwd = tmp.path().join("proj");
        fs::create_dir_all(&cwd).unwrap();

        let extra = tmp.path().join("extra");
        let skill_dir = extra.join("gamma");
        make_skill(&skill_dir, "---\nname: gamma\ndescription: extra\n---\n");

        env::set_var("HOME", tmp.path());
        env::remove_var("TASKPILOT_SKILLS_DIR");
        env::set_current_dir(&cwd).unwrap();

        let skills = discover(&[extra.to_string_lossy().to_string()]).unwrap();
        assert!(skills.iter().any(|s| s.name == "gamma"));
    }

    // ── build_catalog ──────────────────────────────────────

    #[test]
    fn build_catalog_empty() {
        assert!(build_catalog(&[]).is_empty());
    }

    #[test]
    fn build_catalog_multiple() {
        let skills = vec![
            Skill {
                name: "a".into(),
                description: "da".into(),
                path: PathBuf::from("/x"),
            },
            Skill {
                name: "b".into(),
                description: "db".into(),
                path: PathBuf::from("/y"),
            },
        ];
        let cat = build_catalog(&skills);
        assert_eq!(cat, vec![("a".into(), "da".into()), ("b".into(), "db".into())]);
    }

    // ── find_by_name ───────────────────────────────────────

    #[test]
    fn find_by_name_found() {
        let skills = vec![Skill {
            name: "x".into(),
            description: "".into(),
            path: PathBuf::from("/"),
        }];
        assert_eq!(find_by_name(&skills, "x").unwrap().name, "x");
    }

    #[test]
    fn find_by_name_not_found() {
        let skills: Vec<Skill> = vec![];
        assert!(find_by_name(&skills, "z").is_err());
    }

    // ── activate ───────────────────────────────────────────

    #[test]
    fn activate_no_resources() {
        let tmp = TempDir::new().unwrap();
        let skill_dir = tmp.path().join("s");
        make_skill(&skill_dir, "hello world");
        let skill = Skill {
            name: "s".into(),
            description: "".into(),
            path: skill_dir,
        };
        let out = activate(&skill).unwrap();
        assert!(out.contains("hello world"));
        assert!(!out.contains("<skill_resources>"));
        assert!(out.contains("<skill_content"));
        assert!(out.contains("</skill_content>"));
    }

    #[test]
    fn activate_with_resources() {
        let tmp = TempDir::new().unwrap();
        let skill_dir = tmp.path().join("s");
        make_skill(&skill_dir, "body");

        let scripts = skill_dir.join("scripts");
        fs::create_dir_all(&scripts).unwrap();
        fs::write(scripts.join("run.sh"), "#!/bin/bash").unwrap();

        let refs = skill_dir.join("references");
        fs::create_dir_all(&refs).unwrap();
        fs::write(refs.join("doc.txt"), "ref").unwrap();

        let assets = skill_dir.join("assets").join("nested");
        fs::create_dir_all(&assets).unwrap();
        fs::write(assets.join("img.png"), "png").unwrap();

        let skill = Skill {
            name: "s".into(),
            description: "".into(),
            path: skill_dir,
        };
        let out = activate(&skill).unwrap();
        assert!(out.contains("<skill_resources>"));
        assert!(out.contains("scripts/run.sh"));
        assert!(out.contains("references/doc.txt"));
        assert!(out.contains("assets/nested/img.png"));
    }

    // ── enumerate_resources ────────────────────────────────

    #[test]
    fn enumerate_resources_empty() {
        let tmp = TempDir::new().unwrap();
        assert!(enumerate_resources(tmp.path()).is_empty());
    }

    #[test]
    fn enumerate_resources_scripts() {
        let tmp = TempDir::new().unwrap();
        let scripts = tmp.path().join("scripts");
        fs::create_dir_all(&scripts).unwrap();
        fs::write(scripts.join("a.sh"), "").unwrap();
        fs::write(scripts.join("b.py"), "").unwrap();
        let res = enumerate_resources(tmp.path());
        assert_eq!(res.len(), 2);
        assert!(res.iter().any(|r| r == "scripts/a.sh"));
        assert!(res.iter().any(|r| r == "scripts/b.py"));
    }

    #[test]
    fn enumerate_resources_nested_assets() {
        let tmp = TempDir::new().unwrap();
        let nested = tmp.path().join("assets").join("deep").join("dir");
        fs::create_dir_all(&nested).unwrap();
        fs::write(nested.join("file.txt"), "").unwrap();
        let res = enumerate_resources(tmp.path());
        assert_eq!(res.len(), 1);
        assert!(res[0].contains("assets/deep/dir/file.txt"));
    }
}
