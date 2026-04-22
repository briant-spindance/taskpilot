use anyhow::{bail, Context, Result};
use reqwest::blocking::Client;
use serde_json::{json, Value};
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

    let client = Client::new();

    for i in 0..MAX_ITERATIONS {
        let body = json!({
            "model": model,
            "max_tokens": MAX_TOKENS,
            "system": system_prompt,
            "messages": messages,
            "tools": tool_defs,
        });

        let resp = client
            .post(API_URL)
            .header("content-type", "application/json")
            .header("x-api-key", &api_key)
            .header("anthropic-version", API_VERSION)
            .json(&body)
            .send()
            .with_context(|| format!("API call {i}"))?;

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

        // Append assistant message
        messages.push(json!({
            "role": "assistant",
            "content": resp_body["content"]
        }));

        if resp_body["stop_reason"] == "end_turn" {
            return Ok(());
        }

        // Process tool uses
        let mut tool_results = Vec::new();
        if let Some(content) = resp_body["content"].as_array() {
            for block in content {
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
