use anyhow::{bail, Context, Result};
use serde_json::Value;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

/// Return the Anthropic tool definitions, optionally including bash.
pub fn tool_defs(allow_bash: bool) -> Vec<Value> {
    let mut tools: Vec<Value> = Vec::new();

    if allow_bash {
        tools.push(serde_json::json!({
            "name": "bash",
            "description": "Execute a bash command in the working directory. Returns stdout, stderr, and exit code.",
            "input_schema": {
                "type": "object",
                "properties": {
                    "command": { "type": "string", "description": "The bash command to execute" }
                },
                "required": ["command"]
            }
        }));
    }

    tools.push(serde_json::json!({
        "name": "read_file",
        "description": "Read the contents of a file in the working directory.",
        "input_schema": {
            "type": "object",
            "properties": {
                "path": { "type": "string", "description": "Relative path to the file to read" }
            },
            "required": ["path"]
        }
    }));

    tools.push(serde_json::json!({
        "name": "write_file",
        "description": "Write content to a file in the working directory.",
        "input_schema": {
            "type": "object",
            "properties": {
                "path": { "type": "string", "description": "Relative path to the file to write" },
                "content": { "type": "string", "description": "The content to write" }
            },
            "required": ["path", "content"]
        }
    }));

    tools
}

/// Dispatch a tool call and return the text result.
pub fn dispatch(tool_name: &str, input: &Value, work_dir: &Path, allow_bash: bool) -> Result<String> {
    match tool_name {
        "bash" => {
            if !allow_bash {
                bail!("bash tool is disabled. Run with --allow-bash to enable shell commands.")
            }
            exec_bash(input, work_dir)
        }
        "read_file" => exec_read_file(input, work_dir),
        "write_file" => exec_write_file(input, work_dir),
        _ => bail!("unknown tool: {tool_name}"),
    }
}

pub(crate) fn exec_bash(input: &Value, work_dir: &Path) -> Result<String> {
    let command = input["command"]
        .as_str()
        .context("missing 'command' field")?;

    let output = Command::new("bash")
        .arg("-c")
        .arg(command)
        .current_dir(work_dir)
        .output()
        .context("execute bash")?;

    let mut result = String::new();
    result.push_str(&String::from_utf8_lossy(&output.stdout));
    result.push_str(&String::from_utf8_lossy(&output.stderr));

    let code = output.status.code().unwrap_or(-1);
    result.push_str(&format!("\n[exit code: {code}]"));
    Ok(result)
}

pub(crate) fn exec_read_file(input: &Value, work_dir: &Path) -> Result<String> {
    let rel_path = input["path"].as_str().context("missing 'path' field")?;
    let resolved = safe_path(work_dir, rel_path)?;
    let content = fs::read_to_string(&resolved)
        .with_context(|| format!("read file: {}", resolved.display()))?;
    Ok(content)
}

pub(crate) fn exec_write_file(input: &Value, work_dir: &Path) -> Result<String> {
    let rel_path = input["path"].as_str().context("missing 'path' field")?;
    let content = input["content"]
        .as_str()
        .context("missing 'content' field")?;

    let resolved = safe_path(work_dir, rel_path)?;
    if let Some(parent) = resolved.parent() {
        fs::create_dir_all(parent).context("create parent dirs")?;
    }
    fs::write(&resolved, content).context("write file")?;
    Ok(format!("Wrote {} bytes to {rel_path}", content.len()))
}

/// Resolve a relative path within work_dir and reject escapes.
pub(crate) fn safe_path(work_dir: &Path, rel_path: &str) -> Result<PathBuf> {
    let joined = work_dir.join(rel_path);
    let resolved = joined.canonicalize().unwrap_or(joined.clone());
    let work_dir_canon = work_dir.canonicalize().unwrap_or(work_dir.to_path_buf());

    if !resolved.starts_with(&work_dir_canon) {
        // For new files that don't exist yet, check the joined path
        let normalized = normalize_path(&joined);
        let work_norm = normalize_path(work_dir);
        if !normalized.starts_with(&work_norm) {
            bail!("path {rel_path:?} escapes workspace");
        }
        return Ok(joined);
    }
    Ok(resolved)
}

/// Simple path normalization without requiring the path to exist.
pub(crate) fn normalize_path(path: &Path) -> PathBuf {
    let mut components = Vec::new();
    for component in path.components() {
        match component {
            std::path::Component::ParentDir => {
                components.pop();
            }
            std::path::Component::CurDir => {}
            c => components.push(c),
        }
    }
    components.iter().collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use tempfile::TempDir;

    // ── tool_defs ──

    #[test]
    fn tool_defs_with_bash() {
        let defs = tool_defs(true);
        assert_eq!(defs.len(), 3);
        assert_eq!(defs[0]["name"], "bash");
        assert_eq!(defs[1]["name"], "read_file");
        assert_eq!(defs[2]["name"], "write_file");
    }

    #[test]
    fn tool_defs_without_bash() {
        let defs = tool_defs(false);
        assert_eq!(defs.len(), 2);
        assert_eq!(defs[0]["name"], "read_file");
        assert_eq!(defs[1]["name"], "write_file");
    }

    // ── dispatch ──

    #[test]
    fn dispatch_bash_disabled() {
        let tmp = TempDir::new().unwrap();
        let input = json!({"command": "echo hi"});
        let err = dispatch("bash", &input, tmp.path(), false).unwrap_err();
        assert!(err.to_string().contains("bash tool is disabled"));
    }

    #[test]
    fn dispatch_bash_enabled() {
        let tmp = TempDir::new().unwrap();
        let input = json!({"command": "echo hi"});
        let result = dispatch("bash", &input, tmp.path(), true).unwrap();
        assert!(result.contains("hi"));
    }

    #[test]
    fn dispatch_read_file() {
        let tmp = TempDir::new().unwrap();
        fs::write(tmp.path().join("a.txt"), "hello").unwrap();
        let input = json!({"path": "a.txt"});
        let result = dispatch("read_file", &input, tmp.path(), false).unwrap();
        assert_eq!(result, "hello");
    }

    #[test]
    fn dispatch_write_file() {
        let tmp = TempDir::new().unwrap();
        let input = json!({"path": "b.txt", "content": "data"});
        let result = dispatch("write_file", &input, tmp.path(), false).unwrap();
        assert!(result.contains("4 bytes"));
    }

    #[test]
    fn dispatch_unknown_tool() {
        let tmp = TempDir::new().unwrap();
        let err = dispatch("nope", &json!({}), tmp.path(), true).unwrap_err();
        assert!(err.to_string().contains("unknown tool"));
    }

    // ── exec_bash ──

    #[test]
    fn exec_bash_valid() {
        let tmp = TempDir::new().unwrap();
        let result = exec_bash(&json!({"command": "echo hello"}), tmp.path()).unwrap();
        assert!(result.contains("hello"));
        assert!(result.contains("[exit code: 0]"));
    }

    #[test]
    fn exec_bash_missing_command() {
        let tmp = TempDir::new().unwrap();
        let err = exec_bash(&json!({}), tmp.path()).unwrap_err();
        assert!(err.to_string().contains("missing 'command'"));
    }

    #[test]
    fn exec_bash_stderr() {
        let tmp = TempDir::new().unwrap();
        let result = exec_bash(&json!({"command": "echo err >&2"}), tmp.path()).unwrap();
        assert!(result.contains("err"));
    }

    #[test]
    fn exec_bash_nonzero_exit() {
        let tmp = TempDir::new().unwrap();
        let result = exec_bash(&json!({"command": "exit 42"}), tmp.path()).unwrap();
        assert!(result.contains("[exit code: 42]"));
    }

    // ── exec_read_file ──

    #[test]
    fn exec_read_file_valid() {
        let tmp = TempDir::new().unwrap();
        fs::write(tmp.path().join("f.txt"), "content").unwrap();
        let result = exec_read_file(&json!({"path": "f.txt"}), tmp.path()).unwrap();
        assert_eq!(result, "content");
    }

    #[test]
    fn exec_read_file_missing_path() {
        let tmp = TempDir::new().unwrap();
        let err = exec_read_file(&json!({}), tmp.path()).unwrap_err();
        assert!(err.to_string().contains("missing 'path'"));
    }

    #[test]
    fn exec_read_file_nonexistent() {
        let tmp = TempDir::new().unwrap();
        let err = exec_read_file(&json!({"path": "nope.txt"}), tmp.path()).unwrap_err();
        assert!(err.to_string().contains("read file"));
    }

    #[test]
    fn exec_read_file_escape() {
        let tmp = TempDir::new().unwrap();
        let err = exec_read_file(&json!({"path": "../../etc/passwd"}), tmp.path()).unwrap_err();
        assert!(err.to_string().contains("escapes workspace"));
    }

    // ── exec_write_file ──

    #[test]
    fn exec_write_file_valid() {
        let tmp = TempDir::new().unwrap();
        let result = exec_write_file(&json!({"path": "out.txt", "content": "hi"}), tmp.path()).unwrap();
        assert!(result.contains("2 bytes"));
        assert_eq!(fs::read_to_string(tmp.path().join("out.txt")).unwrap(), "hi");
    }

    #[test]
    fn exec_write_file_missing_path() {
        let tmp = TempDir::new().unwrap();
        let err = exec_write_file(&json!({"content": "x"}), tmp.path()).unwrap_err();
        assert!(err.to_string().contains("missing 'path'"));
    }

    #[test]
    fn exec_write_file_missing_content() {
        let tmp = TempDir::new().unwrap();
        let err = exec_write_file(&json!({"path": "x.txt"}), tmp.path()).unwrap_err();
        assert!(err.to_string().contains("missing 'content'"));
    }

    #[test]
    fn exec_write_file_creates_parent_dirs() {
        let tmp = TempDir::new().unwrap();
        let result = exec_write_file(&json!({"path": "a/b/c.txt", "content": "nested"}), tmp.path()).unwrap();
        assert!(result.contains("6 bytes"));
        assert_eq!(fs::read_to_string(tmp.path().join("a/b/c.txt")).unwrap(), "nested");
    }

    #[test]
    fn exec_write_file_escape() {
        let tmp = TempDir::new().unwrap();
        let err = exec_write_file(&json!({"path": "../../evil.txt", "content": "x"}), tmp.path()).unwrap_err();
        assert!(err.to_string().contains("escapes workspace"));
    }

    // ── safe_path ──

    #[test]
    fn safe_path_normal() {
        let tmp = TempDir::new().unwrap();
        fs::write(tmp.path().join("ok.txt"), "").unwrap();
        let result = safe_path(tmp.path(), "ok.txt").unwrap();
        assert!(result.starts_with(tmp.path().canonicalize().unwrap()));
    }

    #[test]
    fn safe_path_escape() {
        let tmp = TempDir::new().unwrap();
        let err = safe_path(tmp.path(), "../../etc/passwd").unwrap_err();
        assert!(err.to_string().contains("escapes workspace"));
    }

    #[test]
    fn safe_path_dotdot_stays_inside() {
        let tmp = TempDir::new().unwrap();
        fs::create_dir(tmp.path().join("sub")).unwrap();
        fs::write(tmp.path().join("file.txt"), "").unwrap();
        let result = safe_path(tmp.path(), "sub/../file.txt").unwrap();
        assert!(result.starts_with(tmp.path().canonicalize().unwrap()));
    }

    #[test]
    fn safe_path_new_file() {
        let tmp = TempDir::new().unwrap();
        // File doesn't exist yet — falls through to normalize check
        let result = safe_path(tmp.path(), "newfile.txt").unwrap();
        assert_eq!(result, tmp.path().join("newfile.txt"));
    }

    // ── normalize_path ──

    #[test]
    fn normalize_path_with_parent() {
        let result = normalize_path(Path::new("/a/b/../c"));
        assert_eq!(result, PathBuf::from("/a/c"));
    }

    #[test]
    fn normalize_path_with_curdir() {
        let result = normalize_path(Path::new("/a/./b"));
        assert_eq!(result, PathBuf::from("/a/b"));
    }

    #[test]
    fn normalize_path_normal() {
        let result = normalize_path(Path::new("/a/b/c"));
        assert_eq!(result, PathBuf::from("/a/b/c"));
    }
}
