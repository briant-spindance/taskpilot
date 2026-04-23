# Refactoring Opportunities

Identified refactoring opportunities for improving maintainability, organized by priority.

## High Impact

### 1. Extract `resolve_run_config()` to eliminate duplication in main.rs
- **Files:** `src/main.rs:184-277`, `src/main.rs:362-413`
- **Issue:** `dispatch_command` and `run_recipe` duplicate prompt resolution (~15 lines), model resolution (~3 lines), stream resolution (~4 lines), bash resolution (~5 lines), and the entire workspace-run-collect pipeline (~15 lines). ~140 lines of near-identical code.
- **Fix:** Create a `RunConfig` struct that holds the resolved prompt, model, stream, bash, inputs, and output_dir. Extract a `resolve_run_config()` function that merges CLI flags, recipe fields, and global config into this struct. Extract `execute_run()` to handle workspace creation, staging, running, and output collection.
- **Status:** DONE

### 2. Unify `DEFAULT_MODEL` constant across all modules
- **Files:** `src/runner.rs:9`, `src/main.rs:219`, `src/main.rs:384`, `src/config.rs:121`
- **Issue:** The model string `"claude-sonnet-4-20250514"` is hardcoded in 4 places. `runner.rs` defines `DEFAULT_MODEL` but `main.rs` and `config.rs` don't use it.
- **Fix:** Make `runner::DEFAULT_MODEL` public and reference it everywhere.
- **Status:** DONE

### 3. Extract constants for repeated path/file names
- **Files:** `src/skill.rs:31-33,63,79,128,145`, `src/install.rs:8,19-20`, `src/registry.rs:165,353-357`
- **Issue:** `"SKILL.md"`, `".agents"`, `"skills"` appear across 3+ modules. If naming conventions change, multiple files must be updated.
- **Fix:** Create shared constants in a `constants.rs` module and reference them throughout.
- **Status:** DONE

### 4. Move business logic out of main.rs into pipeline.rs
- **Files:** `src/main.rs:136-416`
- **Issue:** `dispatch_command` is ~200 lines and `run_recipe` is ~70 lines. `main.rs` orchestrates recipe loading, skill dep resolution, prompt resolution, workspace lifecycle, and runner invocation. This is business logic, not CLI plumbing.
- **Fix:** Extract run pipeline orchestration into a `pipeline.rs` module, leaving `main.rs` responsible only for CLI parsing and dispatch.
- **Status:** DONE

### 5. Consolidate HTTP clients (reqwest vs ureq)
- **Files:** `Cargo.toml:13,22`, `src/runner.rs:204-209`, `src/registry.rs:3`
- **Issue:** Both `reqwest` (blocking API calls in runner.rs and registry.rs) and `ureq` (streaming in runner.rs) are dependencies. This adds binary size and dependency surface.
- **Fix:** Standardize on `ureq` for all HTTP. It supports both blocking and streaming, is lighter weight, and has fewer transitive dependencies.
- **Status:** DONE

## Medium Impact

### 6. Narrow `pub` to `pub(crate)` across the board
- **Files:** `src/runner.rs:15-22`, `src/recipe.rs:20-34`, `src/skill.rs:9-14`, plus many functions
- **Issue:** This is a binary crate — nothing outside the crate can use `pub` items. All `pub` structs, fields, and functions should be `pub(crate)` to communicate intent and prevent accidental coupling.
- **Status:** DONE

### 7. Break up `doctor()` in recipe.rs (~160 lines)
- **Files:** `src/recipe.rs:155-313`
- **Issue:** Single function handles config check, API key check, TOML parsing, and per-recipe validation (prompt, inputs, output_dir, skill_deps, depends_on, allow_bash).
- **Fix:** Extract each validation section into its own function.
- **Status:** TODO

### 8. Deduplicate home directory resolution
- **Files:** `src/skill.rs:162`, `src/install.rs:17`, `src/registry.rs:353`, `src/config.rs:25`
- **Issue:** Four modules each call `std::env::var("HOME")` independently.
- **Fix:** Centralize into a shared `home_dir()` utility (could live in constants.rs or a utils.rs).
- **Status:** DONE

## Low Impact

### 9. Extract shared test fixtures
- **Files:** `src/main.rs:476-501`, `src/runner.rs:392-417`
- **Issue:** `MockApiClient` is defined nearly identically in both modules. Test setup boilerplate (TempDir + env vars) is repeated heavily across test modules.
- **Fix:** Create a `#[cfg(test)] mod test_helpers` or similar shared module.
- **Status:** DONE

### 10. Merge `install.rs` into `registry.rs`
- **Files:** `src/install.rs` (43 lines of non-test code)
- **Issue:** Very thin module; both `install.rs` and `registry.rs` deal with skill installation.
- **Fix:** Move `from_local` and `copy_dir` into `registry.rs` or make `install.rs` the shared installation module.
- **Status:** DONE

### 11. Parse SSE stream with a state struct
- **Files:** `src/runner.rs:216-322`
- **Issue:** `parse_sse_stream` (106 lines) tracks multiple state variables (`current_tool_id`, `current_tool_name`, `current_tool_input_json`). These could be encapsulated in a struct with methods for cleaner state management.
- **Status:** TODO
