# taskpilot

*Sometimes you don't need to talk to your agent — you just need it to do the work.*

Taskpilot is a CLI tool that executes [Agent Skills](https://agentskills.io) as standalone, headless agentic tasks. 

Think of it as Makefile for your AI tasks. Give it a prompt and input files, and it drives a full tool-use loop against the Anthropic API until the task is complete.

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

# Run an ad-hoc task
taskpilot run --prompt "Summarize this data" --input data.csv --output-dir out/

# Run a named recipe
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

[recipes.generate-report]
prompt = """
Analyze cleaned.csv and produce a summary report
in report.md with key metrics and insights
"""
input = ["staging/cleaned.csv"]
output_dir = "output/"
model = "claude-sonnet-4-20250514"
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

Skills are activated by the model, not by the user. taskpilot injects a catalog of all discovered skills into the system prompt. The model reads the catalog, determines which skills are relevant to the current task, and activates them via the `activate_skill` tool — loading the full `SKILL.md` instructions and bundled resources. Multiple skills can be activated in a single session.

## Streaming

Output streams in real-time by default. Text appears as the model generates it. Use `--no-stream` to wait for complete responses (useful in CI/pipelines).

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
```

### Reference

| Source | Purpose |
|--------|---------|
| `~/.local/taskpilot/config.yml` | Global defaults (API key, model, streaming) |
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
6. Drives a tool-use loop (bash, read_file, write_file) against the Anthropic API
7. Collects output files from the workspace to `output_dir`

The agent operates in a sandboxed temp directory. Your filesystem is not modified outside the specified output directory.

## License

MIT
