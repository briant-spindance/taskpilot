# taskpilot

*Sometimes you don't need to talk to your agent — you just need it to do the work.*

A CLI tool that executes [Agent Skills](https://agentskills.io) as standalone, headless agentic tasks. Think of it as a Makefile for AI work — give it a prompt and input files, and it drives a full tool-use loop against the Anthropic API until the task is complete.

taskpilot is a skill *client*, not a skill *host*. It discovers installed skills, assembles context for the model, manages the agentic loop, and stages files in and out of an isolated workspace.

## Install

Requires [Rust](https://rustup.rs).

```bash
git clone <repo-url> && cd skillsrunner

# Local build
./install.sh

# Global install (~/.local/bin)
./install.sh --global
```

## Quick Start

```bash
# First-time setup (creates ~/.local/taskpilot/config.yml)
taskpilot config

# Run an ad-hoc task (--allow-bash enables shell commands)
taskpilot run --prompt "Summarize this data" --input data.csv --output-dir out/ --allow-bash

# Run a named recipe (bash permission is set per-recipe in taskpilot.toml)
taskpilot run sales-report

# Dry run (show resolved config without executing)
taskpilot run sales-report --dry-run
```

## Recipes

Recipes turn one-off prompts into repeatable, version-controlled tasks. Define them in `taskpilot.toml` at the root of your project and run them by name.

```bash
# Scaffold a starter taskpilot.toml
taskpilot init

# List all defined recipes
taskpilot recipes

# Run a recipe
taskpilot run generate-report

# Override recipe values with flags
taskpilot run generate-report --model claude-opus-4-20250514 --output-dir ./custom-output

# Dry run — show resolved config without executing
taskpilot run generate-report --dry-run
```

### Example `taskpilot.toml`

This example defines a three-stage data pipeline. Each recipe is a self-contained task with its own prompt, inputs, and outputs. The `depends_on` field wires them into a chain.

```toml
[recipes.clean-data]
prompt = """
Read input.csv, remove rows with missing values,
and write cleaned.csv
"""
input = ["data/input.csv"]
output_dir = "staging/"
allow_bash = true

[recipes.generate-report]
prompt = """
Analyze cleaned.csv and produce a summary report
in report.md with key metrics and insights
"""
input = ["staging/cleaned.csv"]
output_dir = "output/"
model = "claude-sonnet-4-20250514"
allow_bash = true
skill_deps = ["markdown-report"]
depends_on = ["clean-data"]

[recipes.executive-summary]
prompt_file = "prompts/exec-summary.md"
input = ["output/"]
output_dir = "output/"
depends_on = ["generate-report"]
```

Running `taskpilot run executive-summary` triggers the full chain:

```
taskpilot run executive-summary

  ▶ dependency: clean-data
    Stages data/input.csv into a temp workspace
    Agent cleans the data, writes cleaned.csv
    Output collected to staging/

  ▶ dependency: generate-report
    Checks that the "markdown-report" skill is installed
    Stages staging/cleaned.csv into a fresh workspace
    Agent analyzes the data, writes report.md
    Output collected to output/

  ▶ target: executive-summary
    Stages output/ into a fresh workspace
    Agent reads the report, writes executive-summary.md
    Output collected to output/
```

Each step gets its own isolated workspace. The chain is connected through the filesystem — `clean-data` writes to `staging/`, and `generate-report` reads from `staging/`. This is explicit by design: you control exactly what flows between steps.

You can also run any recipe in the chain directly. Running `taskpilot run generate-report` would only execute `clean-data` then `generate-report`, skipping `executive-summary`.

### Recipe fields

| Field | Description |
|-------|-------------|
| `prompt` | Inline task prompt. Supports TOML multi-line strings (`"""`). |
| `prompt_file` | Path to a file containing the prompt. Use this for longer or shared prompts. |
| `input` | Array of files or directories staged into the workspace before the task runs. |
| `output_dir` | Directory where output files are collected after the task completes. |
| `model` | Anthropic model to use for this recipe. |
| `skill_deps` | Skills that must be installed before the recipe runs. Bare names (e.g. `"pdf"`) are checked locally; remote sources (e.g. `"anthropics/skills/pdf"`) prompt for install if missing. |
| `depends_on` | Array of recipe names that must run before this one. |
| `allow_bash` | Enable the bash tool for this recipe (`true`/`false`). Default: `false`. |

All fields are optional. At minimum, a recipe needs either `prompt` or `prompt_file`.

CLI flags (`--prompt`, `--input`, `--output-dir`, `--model`) always override recipe values, so you can use a recipe as a baseline and tweak individual runs.

### Dependency chaining

Recipes can depend on other recipes via `depends_on`. When you run a recipe, taskpilot:

1. Resolves the full dependency graph using topological sort
2. Detects and rejects circular dependencies
3. Runs each dependency in order before the target recipe

Each recipe in the chain manages its own `input` and `output_dir` independently — there is no automatic wiring between them. This keeps behavior explicit: if `generate-report` depends on `clean-data`, you configure `generate-report` to read from `clean-data`'s output directory.

```bash
# Runs clean-data → generate-report → executive-summary
taskpilot run executive-summary
```

The `doctor` command validates that all `depends_on` references exist and that there are no cycles:

```bash
taskpilot doctor
```

### Skill dependencies

When a recipe declares `skill_deps`, taskpilot checks that each skill is installed before running. If a dependency uses the `owner/repo/skill` format and is missing, taskpilot prompts you to install it globally (`~/.agents/skills/`) or locally (`./.agents/skills/`).

```toml
[recipes.quarterly-deck]
prompt = "Create a quarterly results presentation"
skill_deps = ["anthropics/skills/powerpoint-automation"]
```

## Skills

taskpilot uses [Agent Skills](https://agentskills.io) — a portable, open format for giving AI agents procedural knowledge. Skills are directories containing a `SKILL.md` file with instructions, and optionally bundled scripts, references, and assets.

The [skills.sh](https://skills.sh) registry is a public index of community and official skills. taskpilot integrates with it natively — no Node.js or `npx` required.

### Discovery

taskpilot discovers installed skills from these directories (project-level takes precedence):

```
./.agents/skills/<name>/       # project-level
~/.agents/skills/<name>/       # user-level
~/.taskpilot/skills/<name>/    # taskpilot-native
```

Override with `TASKPILOT_SKILLS_DIR` or `--skills-dir` (repeatable):

```bash
taskpilot run my-recipe --skills-dir ./test/skills --skills-dir ./extra/skills
```

### Finding skills

Search the [skills.sh](https://skills.sh) registry to find skills for your task:

```bash
$ taskpilot skills find "pdf"
  1. anthropics/skills/pdf (⬇ 1234)
     Use this skill whenever the user wants to do anything with PDF files...

  2. acme/doc-tools/pdf-merger (⬇ 89)
     Merge and split PDF documents...
```

```bash
$ taskpilot skills find "excel"
  1. anthropics/skills/excel-automation (⬇ 567)
     Create and manipulate Excel spreadsheets...
```

```bash
$ taskpilot skills find "powerpoint presentation"
  1. anthropics/skills/elite-powerpoint-designer (⬇ 432)
     Create world-class PowerPoint presentations...

  2. anthropics/skills/powerpoint-automation (⬇ 301)
     Create professional PowerPoint presentations from various sources...
```

### Installing skills

Install a skill from the registry using the `owner/repo/skill` format:

```bash
# Install to project (./.agents/skills/)
taskpilot skills add anthropics/skills/pdf

# Install globally (~/.agents/skills/)
taskpilot skills add anthropics/skills/pdf --global
```

taskpilot downloads the skill directly from GitHub — no intermediary package manager needed.

You can also install from a local directory:

```bash
taskpilot install ./path/to/my-skill
```

### Listing and inspecting installed skills

```bash
$ taskpilot skills list
  ● pdf
    /Users/you/.agents/skills/pdf
    Use this skill whenever the user wants to do anything with PDF files.

  ● excel-automation
    /Users/you/.agents/skills/excel-automation
    Create and manipulate Excel spreadsheets.

$ taskpilot skills show pdf
Name:        pdf
Description: Use this skill whenever the user wants to do anything with PDF files.
Path:        /Users/you/.agents/skills/pdf
```

### Skill activation

You don't choose which skills to use — the model does. When taskpilot starts a task, it injects a catalog of all discovered skills (name and description) into the system prompt. The model reads your prompt, decides which skills are relevant, and activates them on its own by calling the `activate_skill` tool. This loads the full `SKILL.md` instructions and any bundled scripts, references, or assets into the conversation.

This means you can install a broad set of skills and let the model pick the right ones for each task. A prompt like *"create a PDF report from this data"* will cause the model to activate a PDF skill if one is installed, without you having to specify it. Multiple skills can be activated in a single session if the task calls for it.

If a task *requires* a specific skill and shouldn't run without it, use `skill_deps` in your recipe. This guarantees the skill is installed before the prompt ever reaches the model — taskpilot will error or prompt you to install it rather than letting the model attempt the task without the right instructions.

## Streaming

Output streams in real-time by default. Text appears as the model generates it. Use `--no-stream` to wait for complete responses (useful in CI/pipelines).

## Agent Tools

During a task, the model has access to these tools:

| Tool | Default | Description |
|------|---------|-------------|
| `read_file` | enabled | Read the contents of a file. Path must be relative to the workspace — attempts to read outside the sandbox are rejected. |
| `write_file` | enabled | Write content to a file. Creates parent directories automatically. Same sandboxing rules as `read_file`. |
| `activate_skill` | enabled | Load a skill by name from the discovered catalog. Returns the full `SKILL.md` instructions and a listing of bundled resources (scripts, references, assets). The model calls this when it determines a skill is relevant to the task. |
| `bash` | **disabled** | Execute any shell command in the workspace directory. Returns stdout, stderr, and exit code. Must be explicitly enabled (see Security below). |

All file operations (`read_file`, `write_file`) are sandboxed to the workspace directory. The agent cannot read or write files outside it.

## Security

### Bash is disabled by default

The `bash` tool gives the agent the ability to run arbitrary shell commands with your user's full permissions. This includes installing packages, accessing the network, reading files outside the workspace, and executing arbitrary code. Because of this, **bash is disabled by default**.

When bash is disabled, the model is told it's unavailable and the tool definition is not sent to the API. If the model somehow attempts a bash call anyway, it's rejected with an error.

### Enabling bash

Most real-world tasks need bash — running Python scripts, processing data, installing dependencies. Enable it explicitly when you trust the prompt, skills, and input files:

```bash
# Per-run via CLI flag
taskpilot run my-task --allow-bash

# Per-recipe in taskpilot.toml
[recipes.generate-report]
prompt = "Analyze the data and produce a report"
allow_bash = true

# Globally in ~/.local/taskpilot/config.yml
allow_bash: true
```

Precedence: `--allow-bash` flag > recipe `allow_bash` field > config.yml `allow_bash` > default (false).

### What's sandboxed, what's not

| Tool | Sandboxed? | Notes |
|------|-----------|-------|
| `read_file` | Yes | Paths are resolved and checked — `../` escapes are rejected |
| `write_file` | Yes | Same path restrictions as `read_file` |
| `bash` | **No** | Full shell access when enabled. Commands run with workspace as cwd, but can reach the entire system. |
| `activate_skill` | N/A | Reads skill files from known directories only |

### Recommendations

- **Leave bash disabled** for tasks that only need to read and write files (summarization, formatting, simple transformations)
- **Enable bash per-recipe** rather than globally — this makes it explicit which tasks have shell access
- **Be cautious with untrusted inputs** — a malicious file (e.g. a CSV with crafted content) could influence the model to run harmful commands when bash is enabled
- **Review skills before installing** — skills are arbitrary instructions that tell the model what to do. A malicious skill could instruct the model to exfiltrate data or run destructive commands
- **Use `--dry-run`** to inspect the resolved configuration (including bash status) before executing

## Commands

| Command | Description |
|---------|-------------|
| `taskpilot run [RECIPE]` | Execute a recipe or ad-hoc task |
| `taskpilot recipes` | List defined recipes |
| `taskpilot doctor` | Validate config and environment |
| `taskpilot init` | Scaffold a new `taskpilot.toml` |
| `taskpilot config` | Interactive setup for global config |
| `taskpilot skills list` | List discovered skills |
| `taskpilot skills show <name>` | Show skill details |
| `taskpilot skills find <query>` | Search the skills.sh registry |
| `taskpilot skills add <source>` | Install a skill from the registry |
| `taskpilot install <path>` | Install a skill from a local directory |

## Configuration

taskpilot loads configuration from multiple sources. Later sources override earlier ones:

1. `~/.local/taskpilot/config.yml` — global user defaults
2. `./.env` — project-level environment variables
3. Environment variables — shell exports
4. CLI flags — highest precedence

### Global config

Run `taskpilot config` to create `~/.local/taskpilot/config.yml` interactively, or create it by hand:

```yaml
# Anthropic API key (fallback if ANTHROPIC_API_KEY env var is not set)
api_key: sk-ant-...

# Default model (overridden by --model flag or recipe model field)
model: claude-sonnet-4-20250514

# Default streaming behavior (overridden by --no-stream flag)
stream: true

# Allow bash tool by default (overridden by --allow-bash flag or recipe field)
allow_bash: false
```

### Reference

| Source | Purpose |
|--------|---------|
| `~/.local/taskpilot/config.yml` | Global defaults (API key, model, streaming, bash) |
| `.env` | Project-level environment variables |
| `ANTHROPIC_API_KEY` | API authentication (env var, `.env`, or config) |
| `TASKPILOT_SKILLS_DIR` | Override user-level skills directory |
| `taskpilot.toml` | Recipe definitions |

## How it works

1. Parses CLI flags or loads a recipe from `taskpilot.toml`
2. Resolves `depends_on` chain and runs dependencies first
3. Checks `skill_deps` are installed (prompts to install remote deps if missing)
4. Creates an isolated temp workspace and stages input files
5. Builds a system prompt with the skill catalog
6. Drives a tool-use loop (`read_file`, `write_file`, and `bash` if enabled) against the Anthropic API
7. Collects output files from the workspace to `output_dir`

The agent operates in a sandboxed temp directory. Your filesystem is not modified outside the specified output directory.

## License

MIT
