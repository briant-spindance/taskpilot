# Project Brief: taskpilot

## Overview

**taskpilot** is a lightweight, open-source CLI tool that executes Agent Skills as standalone agentic tasks. It provides a clean interface for running skills from the command line — accepting a prompt and input/output paths — and drives a full agentic loop against the Anthropic API until the task is complete.

taskpilot is a skill *client*, not a skill *host*. It has no opinions about what skills do or what outputs they produce. Its job is to discover installed skills, assemble the correct context for the model, manage the tool-use loop, and stage files in and out.

---

## Problem Statement

Agent Skills (agentskills.io) define a portable, open format for giving AI agents procedural knowledge and specialized capabilities. Skills are widely used inside interactive agents like Claude Code — but there is no lightweight tool for running a skill as a one-shot, headless, scriptable task.

Teams building document automation, code generation pipelines, and other repeatable AI workflows have no clean way to invoke a skill from a shell script, a CI step, a Makefile, or an orchestration tool like Forge. They are forced to either embed complex API logic directly in their scripts or rely on interactive agents that are not designed for batch or pipeline use.

---

## Goals

- Provide a single, portable binary that executes any Agent Skills-compliant skill as an agentic task
- Conform fully to the Agent Skills specification for discovery, parsing, and activation
- Support file-based inputs and outputs, enabling use in shell scripts and orchestration pipelines
- Remain generic — taskpilot knows nothing about specific skills, domains, or output formats
- Be composable with tools like Forge, Make, and shell scripts without requiring configuration

---

## Non-Goals

- taskpilot does not author or validate skills
- taskpilot does not provide a web UI or interactive session mode
- taskpilot does not implement multi-agent orchestration or skill chaining (that is the orchestrator's job)
- taskpilot does not bundle any specific skills

---

## Users

**Primary:** Developers and engineers building document automation or AI-assisted workflows who want to invoke skills from scripts without standing up a full agent infrastructure.

**Secondary:** Forge task definitions and other orchestration tools that need a standard way to dispatch skill-based tasks.

---

## Core Concepts

### Skill Discovery

taskpilot scans the following locations at startup, in precedence order (project-level overrides user-level):

```
./.agents/skills/<name>/          # project-level, cross-client convention
./.<client>/skills/<name>/        # project-level, client-native (future)
~/.agents/skills/<name>/          # user-level, cross-client convention
~/.taskpilot/skills/<name>/     # user-level, taskpilot-native
```

Any directory containing a `SKILL.md` is recognized as a skill. taskpilot parses the YAML frontmatter to extract `name` and `description`, applying lenient validation per the spec.

### Skill Catalog

At session start, taskpilot builds a catalog from all discovered skills — name and description only — and injects it into the system prompt alongside instructions telling the model how to activate skills. The model reads the catalog, determines which skill(s) are relevant to the task, and activates them by reading the corresponding `SKILL.md`.

### Skill Activation

Activation is model-driven. When the model selects a skill, taskpilot:

1. Reads the full `SKILL.md` body
2. Enumerates bundled resources (`scripts/`, `references/`, `assets/`)
3. Wraps the content in `<skill_content>` tags with a `<skill_resources>` listing
4. Returns the wrapped content to the model as a tool result

Multiple skills may be activated in a single session if the task warrants it.

### Agentic Loop

taskpilot drives a standard tool-use loop:

1. Stages input files into an isolated working directory
2. Sends the system prompt (skill content) and task prompt to the Anthropic API
3. Executes tool calls (bash, read_file, write_file) as they arrive
4. Continues until `stop_reason == end_turn`
5. Collects output files from the working directory and writes them to the specified output path

### File Staging

All inputs are copied into a temporary working directory before the session begins. The agent operates within this sandbox. At completion, taskpilot collects any files written to the working directory and moves them to the output destination.

---

## Interface

```
taskpilot run \
  --skill <name> \
  --prompt "Task description" \
  [--prompt-file path/to/prompt.md] \
  [--input file1 --input file2 ...] \
  --output ./out/
```

**Flags:**

| Flag | Description |
|------|-------------|
| `--prompt` | Task prompt as an inline string |
| `--prompt-file` | Task prompt read from a file |
| `--input` | Input file(s) staged into the working directory (repeatable) |
| `--output` | Directory where output files are written |
| `--model` | Anthropic model to use (default: claude-sonnet-4-20250514) |
| `--dry-run` | Print resolved skill path and assembled prompt; do not execute |

**Subcommands:**

| Command | Description |
|---------|-------------|
| `taskpilot skills list` | List all discovered skills with name and description |
| `taskpilot skills show <name>` | Print the resolved skill path and frontmatter |

---

## Technical Design

**Language:** Go — single binary, no runtime dependencies, easy distribution.

**Key packages:**

- `internal/skill` — discovery, parsing, frontmatter extraction, system prompt assembly
- `internal/runner` — agentic loop, Anthropic API client, tool dispatch
- `internal/tools` — bash executor, read_file, write_file implementations
- `internal/workspace` — working directory lifecycle, input staging, output collection
- `internal/install` — skill install and upgrade from local path
- `cmd/taskpilot` — CLI entry point (Cobra)

**Environment:**

- `ANTHROPIC_API_KEY` — required; no default
- `TASKPILOT_SKILLS_DIR` — optional override for user-level skills path

**Dependencies:** The executing environment must provide whatever the skill requires (Python, Node.js, LibreOffice, etc.). taskpilot does not manage or validate the environment — it executes the tool calls the model issues and surfaces errors if they fail.

---

## Open Questions

- Should there be a timeout flag, or should the loop run to completion unconditionally?
- Should taskpilot emit a structured log of tool calls and outputs (for auditability in pipeline use)?

---

## Success Criteria

- A developer can invoke any Agent Skills-compliant skill from a shell script with a single command
- Outputs land at the specified output path without manual intervention
- The tool is composable — it returns a meaningful exit code and writes errors to stderr
- A Forge task definition can call taskpilot as a standard step in an agentic workflow
- The binary is self-contained with no configuration required beyond `ANTHROPIC_API_KEY`

