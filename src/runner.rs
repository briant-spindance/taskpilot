use anyhow::{bail, Context, Result};
use serde_json::{json, Value};
use std::io::{BufRead, BufReader, Write};
use std::path::Path;

use crate::constants::DEFAULT_MODEL;
use crate::skill::{self, Skill};
use crate::tools;
const API_URL: &str = "https://api.anthropic.com/v1/messages";
const API_VERSION: &str = "2023-06-01";
const MAX_TOKENS: u64 = 8192;
const MAX_ITERATIONS: usize = 200;

pub(crate) struct Config {
    pub(crate) model: String,
    pub(crate) prompt: String,
    pub(crate) skills: Vec<Skill>,
    pub(crate) work_dir: String,
    pub(crate) stream: bool,
    pub(crate) allow_bash: bool,
}

/// Trait abstracting API calls so the agentic loop can be tested without HTTP.
pub(crate) trait ApiClient {
    fn call(
        &self,
        api_key: &str,
        body: &Value,
        stream: bool,
        iteration: usize,
    ) -> Result<(Value, String)>;
}

/// Production implementation that delegates to blocking or streaming HTTP calls.
pub(crate) struct DefaultApiClient;

impl ApiClient for DefaultApiClient {
    fn call(
        &self,
        api_key: &str,
        body: &Value,
        stream: bool,
        iteration: usize,
    ) -> Result<(Value, String)> {
        if stream {
            call_streaming(api_key, body)
        } else {
            call_blocking(api_key, body, iteration)
        }
    }
}

/// Execute the agentic loop to completion.
#[allow(dead_code)]
pub(crate) fn run(cfg: &Config) -> Result<()> {
    run_with_client(cfg, &DefaultApiClient)
}

/// Inner agentic loop parameterised on an [`ApiClient`].
pub(crate) fn run_with_client(cfg: &Config, client: &dyn ApiClient) -> Result<()> {
    let api_key =
        std::env::var("ANTHROPIC_API_KEY").context("ANTHROPIC_API_KEY is required")?;

    let model = if cfg.model.is_empty() {
        DEFAULT_MODEL
    } else {
        &cfg.model
    };

    let system_prompt = build_system_prompt(&cfg.skills, cfg.allow_bash);
    let work_dir = Path::new(&cfg.work_dir);

    let mut messages = vec![json!({
        "role": "user",
        "content": [{ "type": "text", "text": cfg.prompt }]
    })];

    let mut tool_defs: Vec<Value> = tools::tool_defs(cfg.allow_bash);
    tool_defs.push(json!({
        "name": "activate_skill",
        "description": "Load a skill's full instructions and resources. Use this when you determine a skill from the catalog is relevant to the task.",
        "input_schema": {
            "type": "object",
            "properties": {
                "name": { "type": "string", "description": "The name of the skill to activate" }
            },
            "required": ["name"]
        }
    }));

    for i in 0..MAX_ITERATIONS {
        let body = json!({
            "model": model,
            "max_tokens": MAX_TOKENS,
            "system": system_prompt,
            "messages": messages,
            "tools": tool_defs,
            "stream": cfg.stream,
        });

        let (content_blocks, stop_reason) = client.call(&api_key, &body, cfg.stream, i)?;

        // Append assistant message
        messages.push(json!({
            "role": "assistant",
            "content": content_blocks
        }));

        if stop_reason == "end_turn" {
            return Ok(());
        }

        // Process tool uses
        let mut tool_results = Vec::new();
        if let Some(blocks) = content_blocks.as_array() {
            for block in blocks {
                if block["type"] != "tool_use" {
                    continue;
                }
                let tool_name = block["name"].as_str().unwrap_or("");
                let tool_id = block["id"].as_str().unwrap_or("");
                let input = &block["input"];

                let (result, is_error) = if tool_name == "activate_skill" {
                    handle_activate_skill(input, &cfg.skills)
                } else {
                    match tools::dispatch(tool_name, input, work_dir, cfg.allow_bash) {
                        Ok(r) => (r, false),
                        Err(e) => (format!("Error: {e}"), true),
                    }
                };

                let truncated = truncate(&result, 200);
                eprintln!("[tool:{tool_name}] {truncated}");

                tool_results.push(json!({
                    "type": "tool_result",
                    "tool_use_id": tool_id,
                    "content": result,
                    "is_error": is_error,
                }));
            }
        }

        if !tool_results.is_empty() {
            messages.push(json!({
                "role": "user",
                "content": tool_results,
            }));
        }
    }

    bail!("agentic loop exceeded {MAX_ITERATIONS} iterations")
}

/// Non-streaming API call. Returns (content_blocks, stop_reason).
fn call_blocking(api_key: &str, body: &Value, iteration: usize) -> Result<(Value, String)> {
    let body_str = serde_json::to_string(body)?;

    let resp = ureq::post(API_URL)
        .set("content-type", "application/json")
        .set("x-api-key", api_key)
        .set("anthropic-version", API_VERSION)
        .send_string(&body_str);

    match resp {
        Ok(r) => {
            let status = r.status();
            let resp_body: Value = r.into_json().context("parse API response")?;
            parse_blocking_response(status, &resp_body)
        }
        Err(ureq::Error::Status(code, r)) => {
            let resp_body: Value = r.into_json().unwrap_or(json!({"error": "unknown"}));
            parse_blocking_response(code, &resp_body)
        }
        Err(e) => {
            anyhow::bail!("API call {iteration}: {e}")
        }
    }
}

/// Parse a blocking API response. Extracted for testability.
pub(crate) fn parse_blocking_response(status: u16, resp_body: &Value) -> Result<(Value, String)> {
    if !(200..300).contains(&status) {
        bail!("API returned {status}: {resp_body}");
    }

    // Print text output
    if let Some(content) = resp_body["content"].as_array() {
        for block in content {
            if block["type"] == "text" {
                if let Some(text) = block["text"].as_str() {
                    eprintln!("{text}");
                }
            }
        }
    }

    let stop_reason = resp_body["stop_reason"]
        .as_str()
        .unwrap_or("")
        .to_string();

    Ok((resp_body["content"].clone(), stop_reason))
}

/// Streaming API call using SSE. Returns (content_blocks, stop_reason).
fn call_streaming(api_key: &str, body: &Value) -> Result<(Value, String)> {
    let body_str = serde_json::to_string(body)?;

    let resp = ureq::post(API_URL)
        .set("content-type", "application/json")
        .set("x-api-key", api_key)
        .set("anthropic-version", API_VERSION)
        .send_string(&body_str)
        .map_err(|e| anyhow::anyhow!("streaming API call failed: {e}"))?;

    let reader = BufReader::new(resp.into_reader());
    parse_sse_stream(reader)
}

/// Parse SSE stream from a reader. Extracted for testability.
pub(crate) fn parse_sse_stream<R: BufRead>(reader: R) -> Result<(Value, String)> {

    // State for accumulating content blocks
    let mut content_blocks: Vec<Value> = Vec::new();
    let mut stop_reason = String::new();

    // Current block being built
    let mut current_tool_id = String::new();
    let mut current_tool_name = String::new();
    let mut current_tool_input_json = String::new();

    for line in reader.lines() {
        let line = line.context("read SSE line")?;

        // SSE format: lines starting with "data: " contain JSON
        if !line.starts_with("data: ") {
            continue;
        }
        let data = &line[6..];
        if data == "[DONE]" {
            break;
        }

        let event: Value = match serde_json::from_str(data) {
            Ok(v) => v,
            Err(_) => continue,
        };

        let event_type = event["type"].as_str().unwrap_or("");

        match event_type {
            "content_block_start" => {
                let block = &event["content_block"];
                let block_type = block["type"].as_str().unwrap_or("");

                if block_type == "tool_use" {
                    current_tool_id = block["id"].as_str().unwrap_or("").to_string();
                    current_tool_name = block["name"].as_str().unwrap_or("").to_string();
                    current_tool_input_json.clear();
                }
                // For text blocks, we'll accumulate via deltas
                if block_type == "text" {
                    content_blocks.push(json!({
                        "type": "text",
                        "text": ""
                    }));
                }
            }

            "content_block_delta" => {
                let delta = &event["delta"];
                let delta_type = delta["type"].as_str().unwrap_or("");

                if delta_type == "text_delta" {
                    if let Some(text) = delta["text"].as_str() {
                        // Print streaming text immediately
                        eprint!("{text}");
                        let _ = std::io::stderr().flush();

                        // Append to last text block
                        if let Some(last) = content_blocks.last_mut() {
                            if last["type"] == "text" {
                                let existing = last["text"].as_str().unwrap_or("");
                                let new_text = format!("{existing}{text}");
                                last["text"] = json!(new_text);
                            }
                        }
                    }
                } else if delta_type == "input_json_delta" {
                    if let Some(json_chunk) = delta["partial_json"].as_str() {
                        current_tool_input_json.push_str(json_chunk);
                    }
                }
            }

            "content_block_stop" => {
                // If we were accumulating a tool_use block, finalize it
                if !current_tool_id.is_empty() {
                    let input: Value = serde_json::from_str(&current_tool_input_json)
                        .unwrap_or(json!({}));
                    content_blocks.push(json!({
                        "type": "tool_use",
                        "id": current_tool_id,
                        "name": current_tool_name,
                        "input": input,
                    }));
                    current_tool_id.clear();
                    current_tool_name.clear();
                    current_tool_input_json.clear();
                } else {
                    // End of text block — print newline
                    eprintln!();
                }
            }

            "message_delta" => {
                if let Some(sr) = event["delta"]["stop_reason"].as_str() {
                    stop_reason = sr.to_string();
                }
            }

            _ => {}
        }
    }

    Ok((json!(content_blocks), stop_reason))
}

pub(crate) fn handle_activate_skill(input: &Value, skills: &[Skill]) -> (String, bool) {
    let name = match input["name"].as_str() {
        Some(n) => n,
        None => return ("Error: missing 'name' field".into(), true),
    };
    match skill::find_by_name(skills, name) {
        Ok(s) => match skill::activate(s) {
            Ok(content) => (content, false),
            Err(e) => (format!("Error: {e}"), true),
        },
        Err(e) => (format!("Error: {e}"), true),
    }
}

pub(crate) fn build_system_prompt(skills: &[Skill], allow_bash: bool) -> String {
    let mut prompt = String::from(
        "You are an AI assistant executing a task using Agent Skills. \
         You have access to read_file and write_file tools to work in an isolated workspace directory.",
    );

    if allow_bash {
        prompt.push_str(" You also have access to a bash tool for executing shell commands.\n\n");
    } else {
        prompt.push_str(
            " The bash tool is not available for this task. \
             You must accomplish the task using only read_file and write_file. \
             Do not attempt to call the bash tool.\n\n",
        );
    }

    if !skills.is_empty() {
        let catalog = skill::build_catalog(skills);
        prompt.push_str("## Available Skills\n\n");
        prompt.push_str(
            "Use the activate_skill tool to load a skill when the task matches its description.\n\n",
        );
        for (name, desc) in &catalog {
            prompt.push_str(&format!("- **{name}**: {desc}\n"));
        }
        prompt.push('\n');
    }

    prompt.push_str(
        "Complete the task described by the user. When finished, stop and do not ask follow-up questions.\n",
    );
    prompt
}

pub(crate) fn truncate(s: &str, max: usize) -> String {
    let s = s.replace('\n', " ");
    if s.len() <= max {
        s
    } else {
        format!("{}...", &s[..max])
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serial_test::serial;
    use serde_json::json;
    use std::path::PathBuf;

    use crate::testutil::MockApiClient;

    fn make_cfg(work_dir: &str) -> Config {
        Config {
            model: String::new(),
            prompt: "test prompt".into(),
            skills: vec![],
            work_dir: work_dir.into(),
            stream: false,
            allow_bash: false,
        }
    }

    // ---- build_system_prompt tests ----

    #[test]
    fn build_system_prompt_bash_enabled_no_skills() {
        let p = build_system_prompt(&[], true);
        assert!(p.contains("bash tool for executing shell commands"));
        assert!(!p.contains("## Available Skills"));
    }

    #[test]
    fn build_system_prompt_bash_disabled_no_skills() {
        let p = build_system_prompt(&[], false);
        assert!(p.contains("bash tool is not available"));
        assert!(!p.contains("## Available Skills"));
    }

    #[test]
    fn build_system_prompt_with_skills() {
        let skills = vec![Skill {
            name: "test-skill".into(),
            description: "A test skill".into(),
            path: PathBuf::from("/tmp/skill"),
        }];
        let p = build_system_prompt(&skills, true);
        assert!(p.contains("## Available Skills"));
        assert!(p.contains("test-skill"));
        assert!(p.contains("A test skill"));
    }

    #[test]
    fn build_system_prompt_no_skills_no_catalog() {
        let p = build_system_prompt(&[], false);
        assert!(!p.contains("activate_skill"));
    }

    // ---- truncate tests ----

    #[test]
    fn truncate_short_unchanged() {
        assert_eq!(truncate("hello", 10), "hello");
    }

    #[test]
    fn truncate_long_string() {
        let s = "a".repeat(300);
        let result = truncate(&s, 200);
        assert_eq!(result.len(), 203); // 200 + "..."
        assert!(result.ends_with("..."));
    }

    #[test]
    fn truncate_newlines_replaced() {
        let result = truncate("line1\nline2\nline3", 200);
        assert_eq!(result, "line1 line2 line3");
        assert!(!result.contains('\n'));
    }

    // ---- handle_activate_skill tests ----

    #[test]
    fn handle_activate_skill_missing_name() {
        let (msg, is_err) = handle_activate_skill(&json!({}), &[]);
        assert!(is_err);
        assert!(msg.contains("missing 'name' field"));
    }

    #[test]
    fn handle_activate_skill_not_found() {
        let (msg, is_err) =
            handle_activate_skill(&json!({"name": "nonexistent"}), &[]);
        assert!(is_err);
        assert!(msg.contains("Error:"));
    }

    #[test]
    fn handle_activate_skill_found_activate_succeeds() {
        // Create a temp skill directory with a SKILL.md
        let tmp = tempfile::tempdir().unwrap();
        let skill_file = tmp.path().join("SKILL.md");
        std::fs::write(&skill_file, "---\nname: foo\ndescription: bar\n---\nSkill content here").unwrap();

        let skills = vec![Skill {
            name: "foo".into(),
            description: "bar".into(),
            path: tmp.path().to_path_buf(),
        }];
        let (msg, is_err) = handle_activate_skill(&json!({"name": "foo"}), &skills);
        assert!(!is_err);
        assert!(msg.contains("Skill content here"));
    }

    #[test]
    fn handle_activate_skill_found_activate_fails() {
        // Skill directory that has no SKILL.md -> activate will fail
        let tmp = tempfile::tempdir().unwrap();
        let skills = vec![Skill {
            name: "broken".into(),
            description: "desc".into(),
            path: tmp.path().join("nonexistent_subdir"),
        }];
        let (msg, is_err) =
            handle_activate_skill(&json!({"name": "broken"}), &skills);
        assert!(is_err);
        assert!(msg.starts_with("Error:"));
    }

    // ---- run_with_client tests ----

    #[test]
    #[serial]
    fn run_single_turn_end_turn() {
        let tmp = tempfile::tempdir().unwrap();
        std::env::set_var("ANTHROPIC_API_KEY", "test-key");

        let client = MockApiClient::new(vec![Ok((
            json!([{"type": "text", "text": "Done!"}]),
            "end_turn".into(),
        ))]);

        let cfg = make_cfg(tmp.path().to_str().unwrap());
        let result = run_with_client(&cfg, &client);
        assert!(result.is_ok());

        std::env::remove_var("ANTHROPIC_API_KEY");
    }

    #[test]
    #[serial]
    fn run_tool_use_then_end_turn() {
        let tmp = tempfile::tempdir().unwrap();
        // Create a file so read_file tool works
        std::fs::write(tmp.path().join("hello.txt"), "world").unwrap();
        std::env::set_var("ANTHROPIC_API_KEY", "test-key");

        let client = MockApiClient::new(vec![
            // First response: tool_use for read_file
            Ok((
                json!([{
                    "type": "tool_use",
                    "id": "tool_1",
                    "name": "read_file",
                    "input": {"path": "hello.txt"}
                }]),
                "tool_use".into(),
            )),
            // Second response: end_turn
            Ok((
                json!([{"type": "text", "text": "All done"}]),
                "end_turn".into(),
            )),
        ]);

        let cfg = make_cfg(tmp.path().to_str().unwrap());
        let result = run_with_client(&cfg, &client);
        assert!(result.is_ok());

        std::env::remove_var("ANTHROPIC_API_KEY");
    }

    #[test]
    #[serial]
    fn run_activate_skill_tool_use() {
        let tmp = tempfile::tempdir().unwrap();
        std::env::set_var("ANTHROPIC_API_KEY", "test-key");

        // activate_skill with nonexistent skill -> error result, then end_turn
        let client = MockApiClient::new(vec![
            Ok((
                json!([{
                    "type": "tool_use",
                    "id": "tool_1",
                    "name": "activate_skill",
                    "input": {"name": "nonexistent"}
                }]),
                "tool_use".into(),
            )),
            Ok((
                json!([{"type": "text", "text": "ok"}]),
                "end_turn".into(),
            )),
        ]);

        let cfg = make_cfg(tmp.path().to_str().unwrap());
        let result = run_with_client(&cfg, &client);
        assert!(result.is_ok());

        std::env::remove_var("ANTHROPIC_API_KEY");
    }

    #[test]
    #[serial]
    fn run_tool_dispatch_error_is_error_true() {
        let tmp = tempfile::tempdir().unwrap();
        std::env::set_var("ANTHROPIC_API_KEY", "test-key");

        // read_file on nonexistent file -> dispatch error
        let client = MockApiClient::new(vec![
            Ok((
                json!([{
                    "type": "tool_use",
                    "id": "tool_1",
                    "name": "read_file",
                    "input": {"path": "does_not_exist.txt"}
                }]),
                "tool_use".into(),
            )),
            Ok((
                json!([{"type": "text", "text": "done"}]),
                "end_turn".into(),
            )),
        ]);

        let cfg = make_cfg(tmp.path().to_str().unwrap());
        let result = run_with_client(&cfg, &client);
        assert!(result.is_ok());

        std::env::remove_var("ANTHROPIC_API_KEY");
    }

    #[test]
    #[serial]
    fn run_non_tool_use_blocks_skipped() {
        let tmp = tempfile::tempdir().unwrap();
        std::env::set_var("ANTHROPIC_API_KEY", "test-key");

        let client = MockApiClient::new(vec![
            Ok((
                json!([
                    {"type": "text", "text": "thinking..."},
                    {"type": "tool_use", "id": "t1", "name": "read_file", "input": {"path": "x.txt"}}
                ]),
                "tool_use".into(),
            )),
            Ok((
                json!([{"type": "text", "text": "done"}]),
                "end_turn".into(),
            )),
        ]);

        let cfg = make_cfg(tmp.path().to_str().unwrap());
        let result = run_with_client(&cfg, &client);
        assert!(result.is_ok());

        std::env::remove_var("ANTHROPIC_API_KEY");
    }

    #[test]
    #[serial]
    fn run_empty_content_blocks_no_array() {
        let tmp = tempfile::tempdir().unwrap();
        std::env::set_var("ANTHROPIC_API_KEY", "test-key");

        // content_blocks is a string, not an array -> no tool processing, loops again
        let client = MockApiClient::new(vec![
            Ok((json!("not an array"), "tool_use".into())),
            Ok((json!([{"type": "text", "text": "ok"}]), "end_turn".into())),
        ]);

        let cfg = make_cfg(tmp.path().to_str().unwrap());
        let result = run_with_client(&cfg, &client);
        assert!(result.is_ok());

        std::env::remove_var("ANTHROPIC_API_KEY");
    }

    #[test]
    #[serial]
    fn run_max_iterations_exceeded() {
        let tmp = tempfile::tempdir().unwrap();
        std::env::set_var("ANTHROPIC_API_KEY", "test-key");

        // Return "tool_use" stop_reason with no tools forever -> eventually hits max
        let responses: Vec<_> = (0..MAX_ITERATIONS)
            .map(|_| Ok((json!("not array"), "continue".to_string())))
            .collect();

        let client = MockApiClient::new(responses);
        let cfg = make_cfg(tmp.path().to_str().unwrap());
        let result = run_with_client(&cfg, &client);
        assert!(result.is_err());
        assert!(
            result.unwrap_err().to_string().contains("exceeded"),
            "should mention exceeded iterations"
        );

        std::env::remove_var("ANTHROPIC_API_KEY");
    }

    #[test]
    #[serial]
    fn run_missing_api_key() {
        std::env::remove_var("ANTHROPIC_API_KEY");
        let tmp = tempfile::tempdir().unwrap();

        let client = MockApiClient::new(vec![]);
        let cfg = make_cfg(tmp.path().to_str().unwrap());
        let result = run_with_client(&cfg, &client);
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("ANTHROPIC_API_KEY"));
    }

    #[test]
    #[serial]
    fn run_empty_model_uses_default() {
        let tmp = tempfile::tempdir().unwrap();
        std::env::set_var("ANTHROPIC_API_KEY", "test-key");

        let client = MockApiClient::new(vec![Ok((
            json!([{"type": "text", "text": "ok"}]),
            "end_turn".into(),
        ))]);

        let mut cfg = make_cfg(tmp.path().to_str().unwrap());
        cfg.model = String::new(); // empty -> default
        let result = run_with_client(&cfg, &client);
        assert!(result.is_ok());

        std::env::remove_var("ANTHROPIC_API_KEY");
    }

    #[test]
    #[serial]
    fn run_api_client_error_propagated() {
        let tmp = tempfile::tempdir().unwrap();
        std::env::set_var("ANTHROPIC_API_KEY", "test-key");

        let client =
            MockApiClient::new(vec![Err(anyhow::anyhow!("network failure"))]);

        let cfg = make_cfg(tmp.path().to_str().unwrap());
        let result = run_with_client(&cfg, &client);
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("network failure"));

        std::env::remove_var("ANTHROPIC_API_KEY");
    }

    // ── parse_blocking_response tests ─────────────────────────────

    #[test]
    fn parse_blocking_success_with_text() {
        let body = json!({
            "content": [{"type": "text", "text": "hello"}],
            "stop_reason": "end_turn"
        });
        let (content, stop) = parse_blocking_response(200, &body).unwrap();
        assert_eq!(stop, "end_turn");
        assert_eq!(content[0]["text"], "hello");
    }

    #[test]
    fn parse_blocking_success_no_text_blocks() {
        let body = json!({
            "content": [{"type": "tool_use", "id": "t1", "name": "bash", "input": {}}],
            "stop_reason": "tool_use"
        });
        let (content, stop) = parse_blocking_response(200, &body).unwrap();
        assert_eq!(stop, "tool_use");
        assert!(content.as_array().unwrap().len() == 1);
    }

    #[test]
    fn parse_blocking_error_status() {
        let body = json!({"error": {"message": "bad request"}});
        let result = parse_blocking_response(400, &body);
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("400"));
    }

    #[test]
    fn parse_blocking_no_content_array() {
        let body = json!({"stop_reason": "end_turn"});
        let (content, stop) = parse_blocking_response(200, &body).unwrap();
        assert_eq!(stop, "end_turn");
        assert!(content.is_null());
    }

    #[test]
    fn parse_blocking_no_stop_reason() {
        let body = json!({"content": []});
        let (_, stop) = parse_blocking_response(200, &body).unwrap();
        assert_eq!(stop, "");
    }

    #[test]
    fn parse_blocking_text_block_without_text_field() {
        // block has type=text but no text field
        let body = json!({
            "content": [{"type": "text"}],
            "stop_reason": "end_turn"
        });
        let result = parse_blocking_response(200, &body);
        assert!(result.is_ok());
    }

    // ── parse_sse_stream tests ────────────────────────────────────

    #[test]
    fn sse_text_block() {
        let sse = "\
data: {\"type\":\"content_block_start\",\"content_block\":{\"type\":\"text\"}}\n\
\n\
data: {\"type\":\"content_block_delta\",\"delta\":{\"type\":\"text_delta\",\"text\":\"Hello\"}}\n\
\n\
data: {\"type\":\"content_block_delta\",\"delta\":{\"type\":\"text_delta\",\"text\":\" world\"}}\n\
\n\
data: {\"type\":\"content_block_stop\"}\n\
\n\
data: {\"type\":\"message_delta\",\"delta\":{\"stop_reason\":\"end_turn\"}}\n\
\n\
data: [DONE]\n";
        let (blocks, stop) = parse_sse_stream(std::io::Cursor::new(sse)).unwrap();
        assert_eq!(stop, "end_turn");
        let arr = blocks.as_array().unwrap();
        assert_eq!(arr.len(), 1);
        assert_eq!(arr[0]["text"], "Hello world");
    }

    #[test]
    fn sse_tool_use_block() {
        let sse = "\
data: {\"type\":\"content_block_start\",\"content_block\":{\"type\":\"tool_use\",\"id\":\"t1\",\"name\":\"bash\"}}\n\
\n\
data: {\"type\":\"content_block_delta\",\"delta\":{\"type\":\"input_json_delta\",\"partial_json\":\"{\\\"command\\\":\"}}\n\
\n\
data: {\"type\":\"content_block_delta\",\"delta\":{\"type\":\"input_json_delta\",\"partial_json\":\"\\\"echo hi\\\"}\"}}\n\
\n\
data: {\"type\":\"content_block_stop\"}\n\
\n\
data: {\"type\":\"message_delta\",\"delta\":{\"stop_reason\":\"tool_use\"}}\n\
\n\
data: [DONE]\n";
        let (blocks, stop) = parse_sse_stream(std::io::Cursor::new(sse)).unwrap();
        assert_eq!(stop, "tool_use");
        let arr = blocks.as_array().unwrap();
        assert_eq!(arr.len(), 1);
        assert_eq!(arr[0]["type"], "tool_use");
        assert_eq!(arr[0]["name"], "bash");
        assert_eq!(arr[0]["input"]["command"], "echo hi");
    }

    #[test]
    fn sse_non_data_lines_skipped() {
        let sse = "\
event: ping\n\
: comment\n\
data: {\"type\":\"content_block_start\",\"content_block\":{\"type\":\"text\"}}\n\
\n\
data: {\"type\":\"content_block_stop\"}\n\
\n\
data: {\"type\":\"message_delta\",\"delta\":{\"stop_reason\":\"end_turn\"}}\n\
\n\
data: [DONE]\n";
        let (blocks, stop) = parse_sse_stream(std::io::Cursor::new(sse)).unwrap();
        assert_eq!(stop, "end_turn");
        assert_eq!(blocks.as_array().unwrap().len(), 1);
    }

    #[test]
    fn sse_invalid_json_skipped() {
        let sse = "\
data: not-json\n\
data: {\"type\":\"content_block_start\",\"content_block\":{\"type\":\"text\"}}\n\
data: {\"type\":\"content_block_stop\"}\n\
data: {\"type\":\"message_delta\",\"delta\":{\"stop_reason\":\"end_turn\"}}\n\
data: [DONE]\n";
        let (_, stop) = parse_sse_stream(std::io::Cursor::new(sse)).unwrap();
        assert_eq!(stop, "end_turn");
    }

    #[test]
    fn sse_unknown_event_type() {
        let sse = "\
data: {\"type\":\"unknown_event\"}\n\
data: {\"type\":\"message_delta\",\"delta\":{\"stop_reason\":\"end_turn\"}}\n\
data: [DONE]\n";
        let (blocks, stop) = parse_sse_stream(std::io::Cursor::new(sse)).unwrap();
        assert_eq!(stop, "end_turn");
        assert!(blocks.as_array().unwrap().is_empty());
    }

    #[test]
    fn sse_tool_with_invalid_json_input() {
        // Tool input JSON is malformed -> unwrap_or(json!({}))
        let sse = "\
data: {\"type\":\"content_block_start\",\"content_block\":{\"type\":\"tool_use\",\"id\":\"t1\",\"name\":\"test\"}}\n\
data: {\"type\":\"content_block_delta\",\"delta\":{\"type\":\"input_json_delta\",\"partial_json\":\"{invalid\"}}\n\
data: {\"type\":\"content_block_stop\"}\n\
data: {\"type\":\"message_delta\",\"delta\":{\"stop_reason\":\"end_turn\"}}\n\
data: [DONE]\n";
        let (blocks, _) = parse_sse_stream(std::io::Cursor::new(sse)).unwrap();
        let arr = blocks.as_array().unwrap();
        assert_eq!(arr[0]["input"], json!({}));
    }

    #[test]
    fn sse_empty_stream() {
        let sse = "data: [DONE]\n";
        let (blocks, stop) = parse_sse_stream(std::io::Cursor::new(sse)).unwrap();
        assert_eq!(stop, "");
        assert!(blocks.as_array().unwrap().is_empty());
    }

    #[test]
    fn sse_message_delta_no_stop_reason() {
        let sse = "\
data: {\"type\":\"message_delta\",\"delta\":{}}\n\
data: [DONE]\n";
        let (_, stop) = parse_sse_stream(std::io::Cursor::new(sse)).unwrap();
        assert_eq!(stop, "");
    }

    #[test]
    fn sse_text_delta_no_text_field() {
        let sse = "\
data: {\"type\":\"content_block_start\",\"content_block\":{\"type\":\"text\"}}\n\
data: {\"type\":\"content_block_delta\",\"delta\":{\"type\":\"text_delta\"}}\n\
data: {\"type\":\"content_block_stop\"}\n\
data: {\"type\":\"message_delta\",\"delta\":{\"stop_reason\":\"end_turn\"}}\n\
data: [DONE]\n";
        let (blocks, _) = parse_sse_stream(std::io::Cursor::new(sse)).unwrap();
        assert_eq!(blocks.as_array().unwrap()[0]["text"], "");
    }

    #[test]
    fn sse_input_json_delta_no_partial_json() {
        let sse = "\
data: {\"type\":\"content_block_start\",\"content_block\":{\"type\":\"tool_use\",\"id\":\"t1\",\"name\":\"x\"}}\n\
data: {\"type\":\"content_block_delta\",\"delta\":{\"type\":\"input_json_delta\"}}\n\
data: {\"type\":\"content_block_stop\"}\n\
data: [DONE]\n";
        let (blocks, _) = parse_sse_stream(std::io::Cursor::new(sse)).unwrap();
        assert_eq!(blocks.as_array().unwrap()[0]["input"], json!({}));
    }

    #[test]
    fn sse_content_block_start_unknown_type() {
        let sse = "\
data: {\"type\":\"content_block_start\",\"content_block\":{\"type\":\"image\"}}\n\
data: {\"type\":\"content_block_stop\"}\n\
data: {\"type\":\"message_delta\",\"delta\":{\"stop_reason\":\"end_turn\"}}\n\
data: [DONE]\n";
        let (blocks, _) = parse_sse_stream(std::io::Cursor::new(sse)).unwrap();
        // Neither tool_use nor text, so nothing pushed on start, text newline on stop
        assert!(blocks.as_array().unwrap().is_empty());
    }

    #[test]
    fn sse_delta_unknown_type() {
        let sse = "\
data: {\"type\":\"content_block_start\",\"content_block\":{\"type\":\"text\"}}\n\
data: {\"type\":\"content_block_delta\",\"delta\":{\"type\":\"unknown_delta\"}}\n\
data: {\"type\":\"content_block_stop\"}\n\
data: {\"type\":\"message_delta\",\"delta\":{\"stop_reason\":\"end_turn\"}}\n\
data: [DONE]\n";
        let (blocks, _) = parse_sse_stream(std::io::Cursor::new(sse)).unwrap();
        assert_eq!(blocks.as_array().unwrap()[0]["text"], "");
    }

    #[test]
    fn sse_text_delta_without_prior_block() {
        // text_delta arrives but no text block was started — content_blocks is empty
        let sse = "\
data: {\"type\":\"content_block_delta\",\"delta\":{\"type\":\"text_delta\",\"text\":\"orphan\"}}\n\
data: {\"type\":\"content_block_stop\"}\n\
data: {\"type\":\"message_delta\",\"delta\":{\"stop_reason\":\"end_turn\"}}\n\
data: [DONE]\n";
        let (blocks, _) = parse_sse_stream(std::io::Cursor::new(sse)).unwrap();
        // No blocks were started, so orphan text is lost
        assert!(blocks.as_array().unwrap().is_empty());
    }

    #[test]
    fn sse_text_delta_after_tool_block() {
        // text_delta arrives but last block is tool_use, not text
        let sse = "\
data: {\"type\":\"content_block_start\",\"content_block\":{\"type\":\"tool_use\",\"id\":\"t1\",\"name\":\"x\"}}\n\
data: {\"type\":\"content_block_stop\"}\n\
data: {\"type\":\"content_block_delta\",\"delta\":{\"type\":\"text_delta\",\"text\":\"after tool\"}}\n\
data: {\"type\":\"content_block_stop\"}\n\
data: {\"type\":\"message_delta\",\"delta\":{\"stop_reason\":\"end_turn\"}}\n\
data: [DONE]\n";
        let (blocks, _) = parse_sse_stream(std::io::Cursor::new(sse)).unwrap();
        let arr = blocks.as_array().unwrap();
        // Tool block was finalized, text arrives but last block is tool_use (not "text")
        assert_eq!(arr.len(), 1); // only the tool block
        assert_eq!(arr[0]["type"], "tool_use");
    }
}
