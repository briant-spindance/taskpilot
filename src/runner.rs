use anyhow::{bail, Context, Result};
use serde_json::{json, Value};
use std::io::{BufRead, BufReader, Write};
use std::path::Path;

use crate::skill::{self, Skill};
use crate::tools;

const DEFAULT_MODEL: &str = "claude-sonnet-4-20250514";
const API_URL: &str = "https://api.anthropic.com/v1/messages";
const API_VERSION: &str = "2023-06-01";
const MAX_TOKENS: u64 = 8192;
const MAX_ITERATIONS: usize = 200;

pub struct Config {
    pub model: String,
    pub prompt: String,
    pub skills: Vec<Skill>,
    pub work_dir: String,
    pub stream: bool,
}

/// Execute the agentic loop to completion.
pub fn run(cfg: &Config) -> Result<()> {
    let api_key =
        std::env::var("ANTHROPIC_API_KEY").context("ANTHROPIC_API_KEY is required")?;

    let model = if cfg.model.is_empty() {
        DEFAULT_MODEL
    } else {
        &cfg.model
    };

    let system_prompt = build_system_prompt(&cfg.skills);
    let work_dir = Path::new(&cfg.work_dir);

    let mut messages = vec![json!({
        "role": "user",
        "content": [{ "type": "text", "text": cfg.prompt }]
    })];

    let mut tool_defs: Vec<Value> = tools::tool_defs();
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

        let (content_blocks, stop_reason) = if cfg.stream {
            call_streaming(&api_key, &body)?
        } else {
            call_blocking(&api_key, &body, i)?
        };

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
                    match tools::dispatch(tool_name, input, work_dir) {
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
    let client = reqwest::blocking::Client::new();
    let resp = client
        .post(API_URL)
        .header("content-type", "application/json")
        .header("x-api-key", api_key)
        .header("anthropic-version", API_VERSION)
        .json(body)
        .send()
        .with_context(|| format!("API call {iteration}"))?;

    let status = resp.status();
    let resp_body: Value = resp.json().context("parse API response")?;

    if !status.is_success() {
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

fn handle_activate_skill(input: &Value, skills: &[Skill]) -> (String, bool) {
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

fn build_system_prompt(skills: &[Skill]) -> String {
    let mut prompt = String::from(
        "You are an AI assistant executing a task using Agent Skills. \
         You have access to bash, read_file, and write_file tools to work in an isolated workspace directory.\n\n",
    );

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

fn truncate(s: &str, max: usize) -> String {
    let s = s.replace('\n', " ");
    if s.len() <= max {
        s
    } else {
        format!("{}...", &s[..max])
    }
}
