# Architecture: taskpilot

## System Overview

taskpilot is a CLI tool written in Rust that executes Agent Skills as headless, one-shot agentic tasks. It discovers installed skills, assembles context for the Anthropic API, drives a tool-use loop to completion, and stages files in and out of an isolated workspace.

```
┌─────────────────────────────────────────────────────┐
│                    CLI (clap)                        │
│                   src/main.rs                        │
└──────────┬──────────────┬───────────────┬───────────┘
           │              │               │
     ┌─────▼─────┐ ┌─────▼──────┐ ┌──────▼──────┐
     │  skills    │ │    run     │ │  dry-run    │
     │  list/show │ │  command   │ │  command    │
     └─────┬─────┘ └─────┬──────┘ └──────┬──────┘
           │              │               │
     ┌─────▼──────────────▼───────────────▼──────┐
     │              src/skill.rs                  │
     │  Discovery · Parsing · Catalog · Activation│
     └─────────────────────┬─────────────────────┘
                           │
     ┌─────────────────────▼─────────────────────┐
     │            src/workspace.rs                │
     │  Temp dir lifecycle · Input staging ·      │
     │  Output collection                         │
     └─────────────────────┬─────────────────────┘
                           │
     ┌─────────────────────▼─────────────────────┐
     │             src/runner.rs                  │
     │  Agentic loop · Anthropic API client ·     │
     │  System prompt assembly · Tool dispatch    │
     └─────────────────────┬─────────────────────┘
                           │
     ┌─────────────────────▼─────────────────────┐
     │              src/tools.rs                  │
     │  bash · read_file · write_file             │
     └───────────────────────────────────────────┘
```

---

## Module Responsibilities

### `src/main.rs`

CLI entry point using clap (derive API). Defines three command paths:

- `taskpilot run` — execute a skill against a prompt and input files
- `taskpilot skills list` — print discovered skills (name + description)
- `taskpilot skills show <name>` — print resolved path and frontmatter

Parses flags (`--prompt`, `--prompt-file`, `--input`, `--output`, `--model`, `--dry-run`), wires up dependencies, and delegates to library modules.

### `src/skill.rs`

**Discovery.** Scans four directory tiers in precedence order:

1. `./.agents/skills/<name>/` (project-level, cross-client)
2. `./.<client>/skills/<name>/` (project-level, client-native — future)
3. `~/.agents/skills/<name>/` (user-level, cross-client)
4. `~/.taskpilot/skills/<name>/` (user-level, taskpilot-native)

`TASKPILOT_SKILLS_DIR` overrides the user-level path when set. A directory is a valid skill if it contains a `SKILL.md`.

**Parsing.** Extracts YAML frontmatter (`name`, `description`) with lenient validation per the Agent Skills spec. Uses the `serde_yaml` crate.

**Catalog.** Builds an in-memory `Vec<Skill>` with `{name, description, path}` for all discovered skills. Injected into the system prompt so the model can select skills.

**Activation.** On model request, reads the full `SKILL.md` body, enumerates bundled resources (`scripts/`, `references/`, `assets/`), and returns content wrapped in `<skill_content>` / `<skill_resources>` tags.

### `src/runner.rs`

Drives the agentic loop:

1. Assembles the system prompt (skill catalog + activation instructions).
2. Sends messages to the Anthropic API (`/v1/messages`) via `reqwest`.
3. Inspects the response for tool-use blocks.
4. Dispatches each tool call to `src/tools.rs`.
5. Appends tool results to the conversation and loops.
6. Terminates when `stop_reason == "end_turn"`.

Owns the `reqwest::blocking::Client`. Configurable model via `--model` flag (default: `claude-sonnet-4-20250514`). Requires `ANTHROPIC_API_KEY`.

### `src/tools.rs`

Implements the three tools the model can call:

| Tool | Behavior |
|------|----------|
| `bash` | Executes a shell command in the workspace directory via `std::process::Command`, returns stdout/stderr and exit code |
| `read_file` | Reads a file from the workspace, returns contents |
| `write_file` | Writes content to a file in the workspace |

All file operations are scoped to the workspace directory — paths outside the workspace are rejected via canonicalization checks.

### `src/workspace.rs`

Manages the isolated working directory lifecycle:

1. **Create** — `tempfile::tempdir()` to create a fresh temp directory.
2. **Stage inputs** — Copies each `--input` file into the workspace, preserving filenames.
3. **Collect outputs** — After the loop completes, copies files from the workspace to the `--output` directory.
4. **Cleanup** — `TempDir` drop implementation removes the directory automatically.

### `src/install.rs`

Handles skill installation and upgrade from a local path. Copies a skill directory into the appropriate skills location.

---

## Data Flow

```
User invokes CLI
        │
        ▼
  Parse flags & resolve skill
        │
        ▼
  Create workspace (temp dir)
        │
        ▼
  Stage input files ──► workspace/
        │
        ▼
  Build system prompt (catalog + skill content)
        │
        ▼
  ┌─────────────────────────────┐
  │      Agentic Loop           │
  │                             │
  │  Send messages ──► Anthropic│
  │       ◄── tool_use response │
  │  Dispatch tool call         │
  │  Append tool result         │
  │  Repeat until end_turn      │
  └─────────────────────────────┘
        │
        ▼
  Collect output files ──► --output dir
        │
        ▼
  Cleanup workspace, exit with code
```

---

## Key Design Decisions

1. **Single binary, no runtime.** Rust compiles to a static binary with no runtime dependencies. Users install by downloading a single file.

2. **Model-driven skill activation.** taskpilot does not select skills — it presents a catalog and the model decides. This keeps the tool generic and avoids domain-specific logic.

3. **Workspace isolation.** All agent file operations happen in a temp directory. This prevents accidental mutation of the user's filesystem and makes input/output boundaries explicit.

4. **No skill opinions.** taskpilot does not validate skill outputs, enforce schemas, or understand domains. It is a pure execution harness.

5. **Composability.** Meaningful exit codes, stderr for errors, stdout for output. Designed to slot into shell scripts, Makefiles, CI steps, and Forge task definitions without configuration.

---

## Crate Dependencies

| Crate | Purpose |
|-------|---------|
| `clap` (derive) | CLI argument parsing and subcommands |
| `serde` + `serde_json` | JSON serialization for Anthropic API |
| `serde_yaml` | YAML frontmatter parsing in SKILL.md |
| `reqwest` (blocking) | HTTP client for Anthropic API |
| `tempfile` | Workspace temp directory management |
| `anyhow` | Ergonomic error handling |
| `walkdir` | Recursive directory traversal for resource enumeration |

---

## External Dependencies

- **Anthropic API** — the only external service. All model interaction goes through `/v1/messages`.
- **Host environment** — skills may require Python, Node.js, LibreOffice, etc. taskpilot does not manage these; it surfaces errors if tool calls fail.

---

## Configuration

| Source | Purpose |
|--------|---------|
| `ANTHROPIC_API_KEY` | Required. API authentication. |
| `TASKPILOT_SKILLS_DIR` | Optional. Override user-level skills directory. |
| `--model` flag | Optional. Select Anthropic model (default: `claude-sonnet-4-20250514`). |
