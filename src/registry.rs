use anyhow::{bail, Context, Result};
use colored::Colorize;
use reqwest::blocking::Client;
use serde::Deserialize;
use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};

const SEARCH_API: &str = "https://skills.sh/api/search";
const GITHUB_API: &str = "https://api.github.com";
const MAX_RESULTS: usize = 10;

#[derive(Debug, Deserialize)]
struct SearchResponse {
    skills: Vec<SearchResult>,
}

#[derive(Debug, Deserialize)]
struct SearchResult {
    id: String,
    #[allow(dead_code)]
    name: String,
    installs: u64,
    #[allow(dead_code)]
    source: String,
}

#[derive(Debug, Deserialize)]
struct GitHubContent {
    name: String,
    path: String,
    #[serde(rename = "type")]
    content_type: String,
    download_url: Option<String>,
}

/// Search the skills.sh registry and print results.
pub fn find(query: &str) -> Result<()> {
    let client = Client::new();
    let resp = client
        .get(SEARCH_API)
        .query(&[("q", query)])
        .header("user-agent", "taskpilot")
        .send()
        .context("search skills.sh")?;

    if !resp.status().is_success() {
        bail!("skills.sh returned {}", resp.status());
    }

    let data: SearchResponse = resp.json().context("parse search response")?;

    if data.skills.is_empty() {
        println!("{}", "No skills found.".dimmed());
        return Ok(());
    }

    let results: Vec<_> = data.skills.into_iter().take(MAX_RESULTS).collect();

    println!(
        "  {} results for {:?}\n",
        results.len().to_string().bold(),
        query
    );

    for s in &results {
        let installs = format_installs(s.installs);
        println!(
            "  {} {}  {}",
            "●".green(),
            s.source_with_skill().bold(),
            installs.cyan()
        );
        println!(
            "    {}",
            format!("https://skills.sh/{}", s.id).dimmed()
        );
    }

    println!();
    println!(
        "  {} {}",
        "Install:".dimmed(),
        "taskpilot skills add <owner/repo/skill>".bold()
    );

    Ok(())
}

/// Download and install a skill from a GitHub repository.
/// Format: owner/repo@skill or owner/repo (installs all skills from the repo).
pub fn add(source: &str, global: bool) -> Result<()> {
    let (owner, repo, skill_name) = parse_source(source)?;

    let client = Client::builder()
        .user_agent("taskpilot")
        .build()
        .context("build HTTP client")?;

    // Determine the skill directory within the repo.
    // Skills can live at: skills/<name>/, <name>/, or root (if single-skill repo).
    let (remote_path, resolved_name) = resolve_skill_path(&client, &owner, &repo, skill_name.as_deref())?;

    let dest = install_dir(global)?.join(&resolved_name);

    // Remove existing
    let _ = fs::remove_dir_all(&dest);
    fs::create_dir_all(&dest).context("create skill dir")?;

    eprintln!(
        "  {} {} from {}/{}...",
        "Installing".green().bold(),
        resolved_name.bold(),
        owner,
        repo
    );

    // Recursively download the skill directory
    download_dir(&client, &owner, &repo, &remote_path, &dest)?;

    // Verify SKILL.md exists
    if !dest.join("SKILL.md").exists() {
        let _ = fs::remove_dir_all(&dest);
        bail!("downloaded directory does not contain SKILL.md");
    }

    eprintln!(
        "  {} Installed {} to {}",
        "✓".green().bold(),
        resolved_name.bold(),
        dest.display().to_string().dimmed()
    );

    Ok(())
}

/// Parse source string. Accepts:
///   owner/repo@skill
///   owner/repo/skill  (3-part slash format, as shown by `find`)
///   owner/repo         (install from repo root)
fn parse_source(source: &str) -> Result<(String, String, Option<String>)> {
    // First check for @ format
    if let Some((repo_part, skill)) = source.split_once('@') {
        let parts: Vec<&str> = repo_part.split('/').collect();
        if parts.len() != 2 {
            bail!("invalid source format: expected owner/repo@skill, got {source:?}");
        }
        return Ok((parts[0].to_string(), parts[1].to_string(), Some(skill.to_string())));
    }

    // Slash-separated format
    let parts: Vec<&str> = source.split('/').collect();
    match parts.len() {
        2 => Ok((parts[0].to_string(), parts[1].to_string(), None)),
        3 => Ok((parts[0].to_string(), parts[1].to_string(), Some(parts[2].to_string()))),
        _ => bail!(
            "invalid source format: expected owner/repo/skill or owner/repo@skill, got {source:?}"
        ),
    }
}

/// Find where the skill lives in the repo.
fn resolve_skill_path(
    client: &Client,
    owner: &str,
    repo: &str,
    skill_name: Option<&str>,
) -> Result<(String, String)> {
    if let Some(name) = skill_name {
        // Try exact matches first: skills/<name>/, then <name>/ at root
        let candidates = [
            format!("skills/{name}"),
            name.to_string(),
        ];
        for path in &candidates {
            if remote_dir_exists(client, owner, repo, path)? {
                return Ok((path.clone(), name.to_string()));
            }
        }

        // Fuzzy: search common parent directories for a match.
        // Registry IDs often prefix skill names (e.g. "vercel-react-best-practices"
        // for a directory actually named "react-best-practices").
        let search_dirs = ["skills", "plugins", ""];
        for parent in &search_dirs {
            let entries = if parent.is_empty() {
                list_github_dir(client, owner, repo, "")
            } else {
                list_github_dir(client, owner, repo, parent)
            };
            if let Ok(entries) = entries {
                let dirs: Vec<_> = entries
                    .iter()
                    .filter(|e| e.content_type == "dir")
                    .collect();

                // Try: exact match, suffix match, contains match
                let matchers: Vec<Box<dyn Fn(&str) -> bool>> = vec![
                    Box::new(|dir_name: &str| dir_name == name),
                    Box::new(|dir_name: &str| name.ends_with(dir_name) && !dir_name.is_empty()),
                    Box::new(|dir_name: &str| dir_name.ends_with(name)),
                    Box::new(|dir_name: &str| name.contains(dir_name) && dir_name.len() > 3),
                ];

                for matcher in &matchers {
                    if let Some(hit) = dirs.iter().find(|e| matcher(&e.name)) {
                        let path = if parent.is_empty() {
                            hit.name.clone()
                        } else {
                            format!("{parent}/{}", hit.name)
                        };
                        return Ok((path, hit.name.clone()));
                    }
                }
            }
        }

        // Deep search: some repos nest skills under plugins/*/skills/<name>
        for parent in &["plugins"] {
            if let Ok(top_entries) = list_github_dir(client, owner, repo, parent) {
                for top in top_entries.iter().filter(|e| e.content_type == "dir") {
                    let skills_path = format!("{parent}/{}/skills", top.name);
                    if let Ok(skill_entries) = list_github_dir(client, owner, repo, &skills_path) {
                        let dirs: Vec<_> = skill_entries
                            .iter()
                            .filter(|e| e.content_type == "dir")
                            .collect();
                        for d in &dirs {
                            if d.name == name || name.ends_with(&d.name) || d.name.ends_with(name) {
                                let path = format!("{skills_path}/{}", d.name);
                                return Ok((path, d.name.clone()));
                            }
                        }
                    }
                }
            }
        }

        bail!(
            "skill {name:?} not found in {owner}/{repo}. The registry listing may be outdated — \
             check https://github.com/{owner}/{repo} to verify the skill exists."
        );
    }

    // No skill specified — check if root has SKILL.md (single-skill repo)
    let contents = list_github_dir(client, owner, repo, "")?;
    if contents.iter().any(|c| c.name == "SKILL.md") {
        let name = repo.to_string();
        return Ok(("".to_string(), name));
    }

    bail!("no skill name specified and repo root has no SKILL.md; use owner/repo@skill format");
}

fn remote_dir_exists(client: &Client, owner: &str, repo: &str, path: &str) -> Result<bool> {
    let url = format!("{GITHUB_API}/repos/{owner}/{repo}/contents/{path}");
    let resp = client.get(&url).send().context("check remote dir")?;
    Ok(resp.status().is_success())
}

fn list_github_dir(
    client: &Client,
    owner: &str,
    repo: &str,
    path: &str,
) -> Result<Vec<GitHubContent>> {
    let url = if path.is_empty() {
        format!("{GITHUB_API}/repos/{owner}/{repo}/contents")
    } else {
        format!("{GITHUB_API}/repos/{owner}/{repo}/contents/{path}")
    };
    let resp = client.get(&url).send().context("list GitHub dir")?;
    if !resp.status().is_success() {
        bail!("GitHub API returned {} for {url}", resp.status());
    }
    resp.json().context("parse GitHub contents")
}

/// Recursively download a directory from GitHub.
fn download_dir(
    client: &Client,
    owner: &str,
    repo: &str,
    remote_path: &str,
    local_dir: &Path,
) -> Result<()> {
    let contents = list_github_dir(client, owner, repo, remote_path)?;

    for item in &contents {
        let local_path = local_dir.join(&item.name);

        if item.content_type == "dir" {
            fs::create_dir_all(&local_path)?;
            download_dir(client, owner, repo, &item.path, &local_path)?;
        } else if item.content_type == "file" {
            if let Some(ref url) = item.download_url {
                let mut resp = client.get(url).send().context("download file")?;
                let mut content = Vec::new();
                resp.read_to_end(&mut content)?;
                fs::write(&local_path, &content)
                    .with_context(|| format!("write {}", local_path.display()))?;
            }
        }
    }

    Ok(())
}

fn install_dir(global: bool) -> Result<PathBuf> {
    if global {
        let home = std::env::var("HOME").context("resolve home")?;
        Ok(PathBuf::from(home).join(".agents").join("skills"))
    } else {
        let cwd = std::env::current_dir().context("resolve cwd")?;
        Ok(cwd.join(".agents").join("skills"))
    }
}

fn format_installs(n: u64) -> String {
    if n >= 1_000_000 {
        format!("{:.1}M installs", n as f64 / 1_000_000.0)
    } else if n >= 1_000 {
        format!("{:.1}K installs", n as f64 / 1_000.0)
    } else {
        format!("{n} installs")
    }
}

impl SearchResult {
    fn source_with_skill(&self) -> String {
        self.id.clone()
    }
}
