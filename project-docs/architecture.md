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

CLI entry point using clap (derive API). Defines the following command paths:

- `taskpilot run [RECIPE]` — execute a named recipe from `taskpilot.toml`, or an ad-hoc task with flags
- `taskpilot recipes` — list all recipes defined in `taskpilot.toml`
- `taskpilot doctor` — validate `taskpilot.toml` and check environment
- `taskpilot init` — scaffold a new `taskpilot.toml` with an example recipe
- `taskpilot skills list` — print discovered skills (name + description)
- `taskpilot skills show <name>` — print resolved path and frontmatter
- `taskpilot skills find <query>` — search the skills.sh registry
- `taskpilot skills add <source>` — install a skill from GitHub
- `taskpilot install <path>` — install a skill from a local directory

Parses flags (`--prompt`, `--prompt-file`, `--input`, `--output-dir`, `--model`, `--dry-run`, `--no-stream`), loads recipes from `taskpilot.toml` when a recipe name is given, merges CLI flags over recipe defaults, resolves `depends_on` chains before execution, and delegates to library modules.

### `src/recipe.rs`

Manages the recipe system powered by `taskpilot.toml`:

**Loading.** Parses the TOML file from the current directory into a `HashMap<String, Recipe>`.

**Recipe struct.** Each recipe has optional fields: `prompt`, `prompt_file`, `input` (array), `output_dir`, `model`, `skill_deps` (array), and `depends_on` (array of recipe names).

**Skill dependency resolution.** Before executing a recipe, checks each entry in `skill_deps`:
- Bare names (e.g. `"pdf"`) — verifies the skill is installed locally. Errors with install suggestions if missing.
- Remote sources (e.g. `"anthropics/skills/pdf"`) — checks if installed, prompts the user to choose global or local installation if missing, then auto-installs via the registry.

**Recipe chaining (`depends_on`).** Recipes can declare dependencies on other recipes. Before running, taskpilot performs a topological sort to determine execution order and detects circular dependencies. Dependencies run sequentially in order; each manages its own input/output paths independently (execution order only, no automatic output wiring).

**Init.** The `init` command scaffolds a new `taskpilot.toml` with a commented example recipe including `depends_on`.

### `src/registry.rs`

Handles interaction with the skills.sh registry and GitHub:

- **Search** — queries `https://skills.sh/api/search?q=<query>` and displays results.
- **Install** — downloads a skill directory from GitHub (`owner/repo/skill`) into the local or global skills directory. Supports fuzzy name resolution when registry IDs don't exactly match directory names.

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
2. Sends messages to the Anthropic API (`/v1/messages`).
3. **Streaming mode** (default): Uses SSE streaming via `ureq`. Text deltas are printed to stderr in real-time as the model generates them. Tool-use blocks are accumulated until complete, then dispatched.
4. **Non-streaming mode** (`--no-stream`): Uses `reqwest` blocking client. Waits for the full response before printing text and processing tool calls. Useful for CI/pipelines.
5. Inspects the response for tool-use blocks.
6. Dispatches each tool call to `src/tools.rs`.
7. Appends tool results to the conversation and loops.
8. Terminates when `stop_reason == "end_turn"`.

Configurable model via `--model` flag (default: `claude-sonnet-4-20250514`). Requires `ANTHROPIC_API_KEY`.

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
  Parse flags or load recipe from taskpilot.toml
        │
        ▼
  Resolve skill_deps (check local / prompt + install remote)
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
  Collect output files ──► output_dir
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

6. **Recipe system.** `taskpilot.toml` defines named recipes with prompts, inputs, outputs, and skill dependencies. Recipes are invoked by name (`taskpilot run <name>`) and CLI flags override recipe values. Skill dependencies are checked before execution — remote deps prompt for install location.

---

## Crate Dependencies

| Crate | Purpose |
|-------|---------|
| `clap` (derive) | CLI argument parsing and subcommands |
| `serde` + `serde_json` | JSON serialization for Anthropic API |
| `serde_yaml` | YAML frontmatter parsing in SKILL.md |
| `reqwest` (blocking) | HTTP client for Anthropic API (non-streaming) and skills.sh registry |
| `ureq` | HTTP client for Anthropic API streaming (SSE) |
| `tempfile` | Workspace temp directory management |
| `anyhow` | Ergonomic error handling |
| `walkdir` | Recursive directory traversal for resource enumeration |
| `colored` | Terminal color output |
| `dotenvy` | `.env` file loading |
| `toml` | Recipe file parsing |

---

## External Dependencies

- **Anthropic API** — the only external service. All model interaction goes through `/v1/messages`.
- **Host environment** — skills may require Python, Node.js, LibreOffice, etc. taskpilot does not manage these; it surfaces errors if tool calls fail.

---

## Configuration

Configuration is loaded in precedence order (later overrides earlier):

1. `~/.local/taskpilot/config.yml` — global user defaults (API key, model, streaming)
2. `./.env` — project-level environment variables
3. Environment variables — shell exports
4. CLI flags — highest precedence

### `src/config.rs`

Loads and parses `~/.local/taskpilot/config.yml` using `serde_yaml`. Returns a `Config` struct with optional fields: `api_key`, `model`, `stream`. Falls back to defaults if the file is missing or malformed.

| Source | Purpose |
|--------|---------|
| `~/.local/taskpilot/config.yml` | Global defaults: API key, model, streaming behavior. |
| `ANTHROPIC_API_KEY` | Required. API authentication (env var, .env, or config.yml). |
| `TASKPILOT_SKILLS_DIR` | Optional. Override user-level skills directory. |
| `.env` file | Optional. Project-level environment variables. |
| `taskpilot.toml` | Optional. Defines named recipes with prompts, inputs, outputs, and skill deps. |
| `--model` flag | Optional. Select Anthropic model (default: `claude-sonnet-4-20250514`). |
| `--no-stream` flag | Optional. Disable streaming output (wait for full response). |
| `--skills-dir` flag | Optional. Additional skills directories to search (repeatable). |
