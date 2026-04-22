use anyhow::{bail, Context, Result};
use serde_json::Value;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

/// Return the Anthropic tool definitions for the three built-in tools.
pub fn tool_defs() -> Vec<Value> {
    serde_json::json!([
        {
            "name": "bash",
            "description": "Execute a bash command in the working directory. Returns stdout, stderr, and exit code.",
            "input_schema": {
                "type": "object",
                "properties": {
                    "command": { "type": "string", "description": "The bash command to execute" }
                },
                "required": ["command"]
            }
        },
        {
            "name": "read_file",
            "description": "Read the contents of a file in the working directory.",
            "input_schema": {
                "type": "object",
                "properties": {
                    "path": { "type": "string", "description": "Relative path to the file to read" }
                },
                "required": ["path"]
            }
        },
        {
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
        }
    ])
    .as_array()
    .unwrap()
    .clone()
}

/// Dispatch a tool call and return the text result.
pub fn dispatch(tool_name: &str, input: &Value, work_dir: &Path) -> Result<String> {
    match tool_name {
        "bash" => exec_bash(input, work_dir),
        "read_file" => exec_read_file(input, work_dir),
        "write_file" => exec_write_file(input, work_dir),
        _ => bail!("unknown tool: {tool_name}"),
    }
}

fn exec_bash(input: &Value, work_dir: &Path) -> Result<String> {
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

fn exec_read_file(input: &Value, work_dir: &Path) -> Result<String> {
    let rel_path = input["path"].as_str().context("missing 'path' field")?;
    let resolved = safe_path(work_dir, rel_path)?;
    let content = fs::read_to_string(&resolved)
        .with_context(|| format!("read file: {}", resolved.display()))?;
    Ok(content)
}

fn exec_write_file(input: &Value, work_dir: &Path) -> Result<String> {
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
fn safe_path(work_dir: &Path, rel_path: &str) -> Result<PathBuf> {
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
fn normalize_path(path: &Path) -> PathBuf {
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
