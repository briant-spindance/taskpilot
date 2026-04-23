use anyhow::{bail, Context, Result};
use colored::Colorize;
use reqwest::blocking::Client;
use serde::Deserialize;
use serde_json::Value;
use std::fs;
use std::path::{Path, PathBuf};

const SEARCH_API: &str = "https://skills.sh/api/search";
const GITHUB_API: &str = "https://api.github.com";
const MAX_RESULTS: usize = 10;

pub(crate) trait HttpClient {
    fn get_json(&self, url: &str, query: &[(&str, &str)]) -> Result<Value>;
    fn get_bytes(&self, url: &str) -> Result<Vec<u8>>;
    fn check_exists(&self, url: &str) -> Result<bool>;
}

struct RealHttpClient {
    client: Client,
}

impl RealHttpClient {
    fn new() -> Result<Self> {
        let client = Client::builder()
            .user_agent("taskpilot")
            .build()
            .context("build HTTP client")?;
        Ok(Self { client })
    }
}

impl HttpClient for RealHttpClient {
    fn get_json(&self, url: &str, query: &[(&str, &str)]) -> Result<Value> {
        let resp = self
            .client
            .get(url)
            .query(query)
            .send()
            .context("HTTP GET")?;
        if !resp.status().is_success() {
            bail!("HTTP {} for {url}", resp.status());
        }
        resp.json().context("parse JSON")
    }

    fn get_bytes(&self, url: &str) -> Result<Vec<u8>> {
        let resp = self.client.get(url).send().context("HTTP GET bytes")?;
        let bytes = resp.bytes().context("read bytes")?;
        Ok(bytes.to_vec())
    }

    fn check_exists(&self, url: &str) -> Result<bool> {
        let resp = self.client.get(url).send().context("check exists")?;
        Ok(resp.status().is_success())
    }
}

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

impl SearchResult {
    fn source_with_skill(&self) -> String {
        self.id.clone()
    }
}

/// Search the skills.sh registry and print results.
pub fn find(query: &str) -> Result<()> {
    let client = RealHttpClient::new()?;
    find_with_client(query, &client)
}

fn find_with_client(query: &str, client: &dyn HttpClient) -> Result<()> {
    let val = client.get_json(SEARCH_API, &[("q", query)])?;
    let data: SearchResponse = serde_json::from_value(val).context("parse search response")?;

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
pub fn add(source: &str, global: bool) -> Result<()> {
    let client = RealHttpClient::new()?;
    add_with_client(source, global, &client)
}

fn add_with_client(source: &str, global: bool, client: &dyn HttpClient) -> Result<()> {
    let (owner, repo, skill_name) = parse_source(source)?;

    let (remote_path, resolved_name) =
        resolve_skill_path(client, &owner, &repo, skill_name.as_deref())?;

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

    download_dir(client, &owner, &repo, &remote_path, &dest)?;

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
///   owner/repo/skill  (3-part slash format)
///   owner/repo         (install from repo root)
pub(crate) fn parse_source(source: &str) -> Result<(String, String, Option<String>)> {
    if let Some((repo_part, skill)) = source.split_once('@') {
        let parts: Vec<&str> = repo_part.split('/').collect();
        if parts.len() != 2 {
            bail!("invalid source format: expected owner/repo@skill, got {source:?}");
        }
        return Ok((
            parts[0].to_string(),
            parts[1].to_string(),
            Some(skill.to_string()),
        ));
    }

    let parts: Vec<&str> = source.split('/').collect();
    match parts.len() {
        2 => Ok((parts[0].to_string(), parts[1].to_string(), None)),
        3 => Ok((
            parts[0].to_string(),
            parts[1].to_string(),
            Some(parts[2].to_string()),
        )),
        _ => bail!(
            "invalid source format: expected owner/repo/skill or owner/repo@skill, got {source:?}"
        ),
    }
}

fn resolve_skill_path(
    client: &dyn HttpClient,
    owner: &str,
    repo: &str,
    skill_name: Option<&str>,
) -> Result<(String, String)> {
    if let Some(name) = skill_name {
        let candidates = [format!("skills/{name}"), name.to_string()];
        for path in &candidates {
            if remote_dir_exists(client, owner, repo, path)? {
                return Ok((path.clone(), name.to_string()));
            }
        }

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

        // Deep search
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
                            if d.name == name
                                || name.ends_with(&d.name)
                                || d.name.ends_with(name)
                            {
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

    // No skill specified — check if root has SKILL.md
    let contents = list_github_dir(client, owner, repo, "")?;
    if contents.iter().any(|c| c.name == "SKILL.md") {
        let name = repo.to_string();
        return Ok(("".to_string(), name));
    }

    bail!("no skill name specified and repo root has no SKILL.md; use owner/repo@skill format");
}

fn remote_dir_exists(
    client: &dyn HttpClient,
    owner: &str,
    repo: &str,
    path: &str,
) -> Result<bool> {
    let url = format!("{GITHUB_API}/repos/{owner}/{repo}/contents/{path}");
    client.check_exists(&url)
}

fn list_github_dir(
    client: &dyn HttpClient,
    owner: &str,
    repo: &str,
    path: &str,
) -> Result<Vec<GitHubContent>> {
    let url = if path.is_empty() {
        format!("{GITHUB_API}/repos/{owner}/{repo}/contents")
    } else {
        format!("{GITHUB_API}/repos/{owner}/{repo}/contents/{path}")
    };
    let val = client.get_json(&url, &[])?;
    let contents: Vec<GitHubContent> = serde_json::from_value(val).context("parse GitHub contents")?;
    Ok(contents)
}

fn download_dir(
    client: &dyn HttpClient,
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
                let content = client.get_bytes(url)?;
                fs::write(&local_path, &content)
                    .with_context(|| format!("write {}", local_path.display()))?;
            }
        }
    }

    Ok(())
}

pub(crate) fn install_dir(global: bool) -> Result<PathBuf> {
    if global {
        let home = std::env::var("HOME").context("resolve home")?;
        Ok(PathBuf::from(home).join(".agents").join("skills"))
    } else {
        let cwd = std::env::current_dir().context("resolve cwd")?;
        Ok(cwd.join(".agents").join("skills"))
    }
}

pub(crate) fn format_installs(n: u64) -> String {
    if n >= 1_000_000 {
        format!("{:.1}M installs", n as f64 / 1_000_000.0)
    } else if n >= 1_000 {
        format!("{:.1}K installs", n as f64 / 1_000.0)
    } else {
        format!("{n} installs")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serial_test::serial;
    use std::cell::RefCell;
    use std::collections::VecDeque;

    #[derive(Clone, Debug)]
    enum MockResponse {
        Json(Value),
        Bytes(Vec<u8>),
        Exists(bool),
        Err(String),
    }

    struct MockHttpClient {
        responses: RefCell<VecDeque<MockResponse>>,
    }

    impl MockHttpClient {
        fn new(responses: Vec<MockResponse>) -> Self {
            Self {
                responses: RefCell::new(responses.into()),
            }
        }

        fn pop(&self) -> MockResponse {
            self.responses
                .borrow_mut()
                .pop_front()
                .expect("MockHttpClient: no more responses queued")
        }
    }

    impl HttpClient for MockHttpClient {
        fn get_json(&self, _url: &str, _query: &[(&str, &str)]) -> Result<Value> {
            match self.pop() {
                MockResponse::Json(v) => Ok(v),
                MockResponse::Err(e) => bail!("{e}"),
                other => panic!("expected Json or Err, got {other:?}"),
            }
        }

        fn get_bytes(&self, _url: &str) -> Result<Vec<u8>> {
            match self.pop() {
                MockResponse::Bytes(b) => Ok(b),
                MockResponse::Err(e) => bail!("{e}"),
                other => panic!("expected Bytes or Err, got {other:?}"),
            }
        }

        fn check_exists(&self, _url: &str) -> Result<bool> {
            match self.pop() {
                MockResponse::Exists(b) => Ok(b),
                MockResponse::Err(e) => bail!("{e}"),
                other => panic!("expected Exists or Err, got {other:?}"),
            }
        }
    }

    // ── parse_source ──────────────────────────────────────────────

    #[test]
    fn parse_source_owner_repo() {
        let (o, r, s) = parse_source("owner/repo").unwrap();
        assert_eq!(o, "owner");
        assert_eq!(r, "repo");
        assert!(s.is_none());
    }

    #[test]
    fn parse_source_owner_repo_skill_slash() {
        let (o, r, s) = parse_source("owner/repo/skill").unwrap();
        assert_eq!(o, "owner");
        assert_eq!(r, "repo");
        assert_eq!(s.unwrap(), "skill");
    }

    #[test]
    fn parse_source_owner_repo_at_skill() {
        let (o, r, s) = parse_source("owner/repo@skill").unwrap();
        assert_eq!(o, "owner");
        assert_eq!(r, "repo");
        assert_eq!(s.unwrap(), "skill");
    }

    #[test]
    fn parse_source_invalid_at_format() {
        let err = parse_source("invalid@format/too/many").unwrap_err();
        assert!(err.to_string().contains("invalid source format"));
    }

    #[test]
    fn parse_source_single_part() {
        let err = parse_source("one").unwrap_err();
        assert!(err.to_string().contains("invalid source format"));
    }

    #[test]
    fn parse_source_too_many_slashes() {
        let err = parse_source("a/b/c/d").unwrap_err();
        assert!(err.to_string().contains("invalid source format"));
    }

    // ── format_installs ───────────────────────────────────────────

    #[test]
    fn format_installs_millions() {
        assert_eq!(format_installs(2_500_000), "2.5M installs");
    }

    #[test]
    fn format_installs_thousands() {
        assert_eq!(format_installs(1_500), "1.5K installs");
    }

    #[test]
    fn format_installs_small() {
        assert_eq!(format_installs(42), "42 installs");
    }

    #[test]
    fn format_installs_boundary_million() {
        assert_eq!(format_installs(1_000_000), "1.0M installs");
    }

    #[test]
    fn format_installs_boundary_thousand() {
        assert_eq!(format_installs(1_000), "1.0K installs");
    }

    #[test]
    fn format_installs_zero() {
        assert_eq!(format_installs(0), "0 installs");
    }

    // ── install_dir ───────────────────────────────────────────────

    #[test]
    #[serial]
    fn install_dir_global() {
        std::env::set_var("HOME", "/tmp/fakehome");
        let p = install_dir(true).unwrap();
        assert_eq!(p, PathBuf::from("/tmp/fakehome/.agents/skills"));
    }

    #[test]
    #[serial]
    fn install_dir_local() {
        let tmp = tempfile::TempDir::new().unwrap();
        let canonical = tmp.path().canonicalize().unwrap();
        std::env::set_current_dir(&canonical).unwrap();
        let p = install_dir(false).unwrap();
        assert_eq!(p, canonical.join(".agents").join("skills"));
    }

    // ── find_with_client ──────────────────────────────────────────

    #[test]
    fn find_with_client_results() {
        let data = serde_json::json!({
            "skills": [{
                "id": "owner/repo/skill",
                "name": "skill",
                "installs": 1234,
                "source": "owner/repo"
            }]
        });
        let mock = MockHttpClient::new(vec![MockResponse::Json(data)]);
        find_with_client("test", &mock).unwrap();
    }

    #[test]
    fn find_with_client_empty() {
        let data = serde_json::json!({"skills": []});
        let mock = MockHttpClient::new(vec![MockResponse::Json(data)]);
        find_with_client("nothing", &mock).unwrap();
    }

    #[test]
    fn find_with_client_http_error() {
        let mock = MockHttpClient::new(vec![MockResponse::Err("server error".into())]);
        let err = find_with_client("q", &mock).unwrap_err();
        assert!(err.to_string().contains("server error"));
    }

    // ── remote_dir_exists ─────────────────────────────────────────

    #[test]
    fn remote_dir_exists_true() {
        let mock = MockHttpClient::new(vec![MockResponse::Exists(true)]);
        assert!(remote_dir_exists(&mock, "o", "r", "p").unwrap());
    }

    #[test]
    fn remote_dir_exists_false() {
        let mock = MockHttpClient::new(vec![MockResponse::Exists(false)]);
        assert!(!remote_dir_exists(&mock, "o", "r", "p").unwrap());
    }

    // ── list_github_dir ───────────────────────────────────────────

    #[test]
    fn list_github_dir_success() {
        let data = serde_json::json!([
            {"name": "foo", "path": "foo", "type": "file", "download_url": "http://x"}
        ]);
        let mock = MockHttpClient::new(vec![MockResponse::Json(data)]);
        let entries = list_github_dir(&mock, "o", "r", "").unwrap();
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].name, "foo");
    }

    #[test]
    fn list_github_dir_with_path() {
        let data = serde_json::json!([]);
        let mock = MockHttpClient::new(vec![MockResponse::Json(data)]);
        let entries = list_github_dir(&mock, "o", "r", "sub/dir").unwrap();
        assert!(entries.is_empty());
    }

    #[test]
    fn list_github_dir_error() {
        let mock = MockHttpClient::new(vec![MockResponse::Err("404".into())]);
        assert!(list_github_dir(&mock, "o", "r", "x").is_err());
    }

    // ── download_dir ──────────────────────────────────────────────

    #[test]
    fn download_dir_files_and_dirs() {
        let tmp = tempfile::tempdir().unwrap();
        // First call: list root -> one file + one dir
        let root_listing = serde_json::json!([
            {"name": "README.md", "path": "s/README.md", "type": "file", "download_url": "http://dl/readme"},
            {"name": "sub", "path": "s/sub", "type": "dir", "download_url": null}
        ]);
        // get_bytes for README.md
        // list sub dir -> one file
        let sub_listing = serde_json::json!([
            {"name": "data.txt", "path": "s/sub/data.txt", "type": "file", "download_url": "http://dl/data"}
        ]);
        let mock = MockHttpClient::new(vec![
            MockResponse::Json(root_listing),
            MockResponse::Bytes(b"# README".to_vec()),
            MockResponse::Json(sub_listing),
            MockResponse::Bytes(b"hello".to_vec()),
        ]);
        download_dir(&mock, "o", "r", "s", tmp.path()).unwrap();
        assert_eq!(fs::read_to_string(tmp.path().join("README.md")).unwrap(), "# README");
        assert_eq!(fs::read_to_string(tmp.path().join("sub/data.txt")).unwrap(), "hello");
    }

    #[test]
    fn download_dir_file_without_download_url() {
        let tmp = tempfile::tempdir().unwrap();
        let listing = serde_json::json!([
            {"name": "ghost.bin", "path": "ghost.bin", "type": "file", "download_url": null}
        ]);
        let mock = MockHttpClient::new(vec![MockResponse::Json(listing)]);
        download_dir(&mock, "o", "r", "", tmp.path()).unwrap();
        assert!(!tmp.path().join("ghost.bin").exists());
    }

    // ── resolve_skill_path ────────────────────────────────────────

    #[test]
    fn resolve_skill_path_exact_skills_dir() {
        // check_exists("skills/myskill") -> true
        let mock = MockHttpClient::new(vec![MockResponse::Exists(true)]);
        let (path, name) = resolve_skill_path(&mock, "o", "r", Some("myskill")).unwrap();
        assert_eq!(path, "skills/myskill");
        assert_eq!(name, "myskill");
    }

    #[test]
    fn resolve_skill_path_exact_root_dir() {
        // check_exists("skills/myskill") -> false
        // check_exists("myskill") -> true
        let mock = MockHttpClient::new(vec![
            MockResponse::Exists(false),
            MockResponse::Exists(true),
        ]);
        let (path, name) = resolve_skill_path(&mock, "o", "r", Some("myskill")).unwrap();
        assert_eq!(path, "myskill");
        assert_eq!(name, "myskill");
    }

    #[test]
    fn resolve_skill_path_fuzzy_suffix_match() {
        // Both exact checks fail, then fuzzy search in "skills" dir finds suffix match
        let skills_listing = serde_json::json!([
            {"name": "react-best-practices", "path": "skills/react-best-practices", "type": "dir", "download_url": null}
        ]);
        let mock = MockHttpClient::new(vec![
            MockResponse::Exists(false), // skills/vercel-react-best-practices
            MockResponse::Exists(false), // vercel-react-best-practices
            MockResponse::Json(skills_listing), // list skills/
        ]);
        // name = "vercel-react-best-practices", dir = "react-best-practices"
        // suffix matcher: name.ends_with(dir_name) -> "vercel-react-best-practices".ends_with("react-best-practices") = true
        let (path, name) =
            resolve_skill_path(&mock, "o", "r", Some("vercel-react-best-practices")).unwrap();
        assert_eq!(path, "skills/react-best-practices");
        assert_eq!(name, "react-best-practices");
    }

    #[test]
    fn resolve_skill_path_no_skill_root_has_skillmd() {
        // No skill specified, root listing includes SKILL.md
        let root = serde_json::json!([
            {"name": "SKILL.md", "path": "SKILL.md", "type": "file", "download_url": "http://x"}
        ]);
        let mock = MockHttpClient::new(vec![MockResponse::Json(root)]);
        let (path, name) = resolve_skill_path(&mock, "o", "myrepo", None).unwrap();
        assert_eq!(path, "");
        assert_eq!(name, "myrepo");
    }

    #[test]
    fn resolve_skill_path_no_skill_no_skillmd() {
        let root = serde_json::json!([
            {"name": "README.md", "path": "README.md", "type": "file", "download_url": "http://x"}
        ]);
        let mock = MockHttpClient::new(vec![MockResponse::Json(root)]);
        let err = resolve_skill_path(&mock, "o", "r", None).unwrap_err();
        assert!(err.to_string().contains("no skill name specified"));
    }

    #[test]
    fn resolve_skill_path_not_found() {
        // Both exact checks fail, all fuzzy dir listings fail
        let mock = MockHttpClient::new(vec![
            MockResponse::Exists(false),
            MockResponse::Exists(false),
            MockResponse::Err("404".into()), // skills
            MockResponse::Err("404".into()), // plugins
            MockResponse::Err("404".into()), // root
            MockResponse::Err("404".into()), // deep: plugins
        ]);
        let err = resolve_skill_path(&mock, "o", "r", Some("ghost")).unwrap_err();
        assert!(err.to_string().contains("not found"));
    }

    // ── add_with_client ───────────────────────────────────────────

    #[test]
    #[serial]
    fn add_with_client_success() {
        let tmp = tempfile::tempdir().unwrap();
        std::env::set_var("HOME", tmp.path().to_str().unwrap());

        // parse_source -> owner/repo@myskill
        // resolve: check_exists skills/myskill -> true
        // list skills/myskill -> one file SKILL.md
        let listing = serde_json::json!([
            {"name": "SKILL.md", "path": "skills/myskill/SKILL.md", "type": "file", "download_url": "http://dl/skill"}
        ]);
        let mock = MockHttpClient::new(vec![
            MockResponse::Exists(true),       // resolve: skills/myskill exists
            MockResponse::Json(listing),       // download_dir list
            MockResponse::Bytes(b"# Skill".to_vec()), // download SKILL.md
        ]);
        add_with_client("owner/repo@myskill", true, &mock).unwrap();

        let skill_md = tmp.path().join(".agents/skills/myskill/SKILL.md");
        assert!(skill_md.exists());
        assert_eq!(fs::read_to_string(skill_md).unwrap(), "# Skill");
    }

    #[test]
    #[serial]
    fn add_with_client_no_skillmd_cleanup() {
        let tmp = tempfile::tempdir().unwrap();
        std::env::set_var("HOME", tmp.path().to_str().unwrap());

        // resolve finds it, but download yields no SKILL.md
        let listing = serde_json::json!([
            {"name": "README.md", "path": "skills/bad/README.md", "type": "file", "download_url": "http://dl/rm"}
        ]);
        let mock = MockHttpClient::new(vec![
            MockResponse::Exists(true),
            MockResponse::Json(listing),
            MockResponse::Bytes(b"readme".to_vec()),
        ]);
        let err = add_with_client("owner/repo@bad", true, &mock).unwrap_err();
        assert!(err.to_string().contains("does not contain SKILL.md"));
        // dir should be cleaned up
        assert!(!tmp.path().join(".agents/skills/bad").exists());
    }

    #[test]
    #[serial]
    fn add_with_client_replaces_existing() {
        let tmp = tempfile::tempdir().unwrap();
        std::env::set_var("HOME", tmp.path().to_str().unwrap());

        // Pre-create an existing skill dir
        let existing = tmp.path().join(".agents/skills/myskill");
        fs::create_dir_all(&existing).unwrap();
        fs::write(existing.join("old.txt"), "old").unwrap();

        let listing = serde_json::json!([
            {"name": "SKILL.md", "path": "skills/myskill/SKILL.md", "type": "file", "download_url": "http://dl/s"}
        ]);
        let mock = MockHttpClient::new(vec![
            MockResponse::Exists(true),
            MockResponse::Json(listing),
            MockResponse::Bytes(b"# New".to_vec()),
        ]);
        add_with_client("owner/repo@myskill", true, &mock).unwrap();

        // old.txt should be gone, SKILL.md should exist
        assert!(!existing.join("old.txt").exists());
        assert!(existing.join("SKILL.md").exists());
    }

    // ── SearchResult::source_with_skill ───────────────────────────

    #[test]
    fn source_with_skill_returns_id() {
        let sr = SearchResult {
            id: "foo/bar/baz".into(),
            name: "baz".into(),
            installs: 0,
            source: "foo/bar".into(),
        };
        assert_eq!(sr.source_with_skill(), "foo/bar/baz");
    }
}
