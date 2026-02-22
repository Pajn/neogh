AGENTS for neogh (Neovim PR Comments Sidebar Plugin)
=================================================

Purpose
-------
This document defines the agent roles, responsibilities, and workflows to be used when working on the neogh repository. It is tailored to the project's structure (Rust Neovim plugin using nvim-oxi) and the team's Autonomous Agent model (Commander, Planner, Worker, Reviewer).

High-level project summary
--------------------------
- Plugin name: neogh — a Neovim PR comments and workflow status sidebar plugin
- Language: Rust
- Entry point: src/lib.rs (exports commands :PRComments, :PRActions, :PRCommentsClose to Lua)
- Key modules: 
  - src/github/* (GraphQL API integration, PR detection, comment fetching, chain detection, workflow status)
  - src/ui/* (sidebar, buffer rendering, navigation, actions buffer)
  - src/types/* (shared comment types, sidebar mode)
  - src/actions.rs (actions mode navigation and types)

Agent Roles (summary)
---------------------
- Commander (orchestrator)
  - Owns mission progress and high-level coordination.
  - Reads project context, creates and updates .opencode/context.md, instructs Planner.
  - Delegates independent work to Worker agents and assigns Reviewers for verification.
  - NEVER implements feature code directly; never mark tasks complete — Reviewer does.

- Planner
  - Performs discovery and research (dependencies, GH CLI usage, Rust crate configuration, FFI details).
  - Produces the structured .opencode/todo.md task plan (milestones → tasks → leaf steps).
  - Breaks complex tasks into parallel groups mapped to repository layout.
  - Produces exact instructions for Worker agents, including build/test commands and verification steps.

- Worker
  - Implements code and documentation changes in the repository (src/, lua/, tests, docs).
  - Uses structural tools (ast_search/ast_replace) for safe refactors when needed.
  - BEFORE starting: update .opencode/work-log.md with assigned file(s) and session id.
  - AFTER making changes: run local verification (cargo check/build/test, lsp diagnostics), capture outputs, and update .opencode/work-log.md and unit test records in .opencode/unit-tests/.
  - Provide precise evidence (patch summary, build/test output, lsp diagnostics) for Reviewer.
  - Do NOT create git commits unless explicitly requested by the user (follow Git Safety Protocol).

- Reviewer
  - Responsible for evidence-based verification of Worker changes.
  - Runs the project's build and test commands and lsp_diagnostics on changed files.
  - Performs integration checks (plugin load & runtime behavior) and writes reproducible repro steps for any failures.
  - Writes sync issues to .opencode/sync-issues.md and only marks .opencode/todo.md leaf items as [x] when proof is present.

How these agents map to this repository
--------------------------------------
- Planner -> discovery + task decomposition
  - Read: plan.md, Cargo.toml, src/lib.rs, src/github/*, src/ui/*, src/actions.rs, lua/
  - Produce: .opencode/todo.md (parallel groups: `github/`, `ui/`, `types+lib.rs`, `actions.rs`)

- Worker groups (parallelizable)
  - G1: github/ (auth.rs, pr.rs, comments.rs, graphql.rs, chain.rs, workflow.rs) — GraphQL integration and fetching
  - G2: ui/ (sidebar.rs, buffer.rs, navigation.rs, actions_buffer.rs) — Neovim UI + buffer rendering
  - G3: types/ + lib.rs + actions.rs — shared types, exports and lua binding glue

- Reviewer -> Verify build, diagnostics, and runtime behavior after Workers finish each leaf task

Recommended commands and checks (for Workers & Reviewers)
--------------------------------------------------------
- Build: cargo build
- Release build (produce compiled artifact): cargo build --release && cp target/release/libneogh.dylib lua/neogh.so
- Type / static checks: cargo check
- Tests: cargo test
- Lint/format (optional): cargo clippy, cargo fmt -- --check
- Inspect crate-type & FFI: check Cargo.toml for `crate-type = ["cdylib"|"dylib"]` and inspect target/ directory for produced artifacts to confirm expected lua/neogh.so
- Runtime verification (manual): Open Neovim in the repository, ensure the compiled library is loadable, and test all keymaps (j/k, <CR>, q, za, r, R, [p, ]p, <Tab>)

Evidence required from Workers before Reviewer verification
---------------------------------------------------------
1. Patch summary and list of modified files
2. Local build output (cargo build output or summary) and whether build succeeded
3. lsp_diagnostics results (errors/warnings for changed files)
4. Unit test output (if applicable)
5. .opencode/work-log.md updated (session id, files, status)

Reviewer verification checklist
------------------------------
1. Re-run cargo check/build/test; verify identical or better diagnostics than Worker provided
2. Run lsp_diagnostics on modified files (make sure there are no new errors)
3. Follow Worker-provided runtime steps to reproduce the change in Neovim; confirm UI and CLI behaviors
4. If everything passes, mark the leaf task as [x] in .opencode/todo.md and add integration notes to .opencode/integration-status.md
5. If failures are observed, write precise sync issues to .opencode/sync-issues.md with exact file references and suggested fixes

Parallelism guidance
--------------------
- Independent groups (github/, ui/, types/lib, actions.rs) can be worked on concurrently.
- When a Worker changes public types (src/types/*) or lib.rs exports, notify dependent Workers and Reviewer because these are integration boundary changes.

Safety & commit rules
---------------------
- Workers should NOT commit or push changes unless explicitly requested by the user.
- When a commit is requested, follow the Git Safety Protocol: do not amend commits or force push without explicit user approval.

Technical Discoveries
---------------------
### FFI Panic Handling
- nvim-oxi plugin functions are `extern "C"` and cannot unwind
- Any panic crossing this FFI boundary causes "panic in a function that cannot unwind" error
- **Fix**: Wrap all Lua function exports and autocmd callbacks with `std::panic::catch_unwind`

### RefCell Re-entrancy
- Autocmd callbacks (e.g., cursor tracking) can fire while holding a `RefCell` borrow
- This causes `borrow_mut()` to panic with "already borrowed" error
- **Fix**: Use `try_borrow_mut()` instead and handle the `Err` case gracefully

### Line Number Indexing
- Neovim's `get_cursor()` returns 0-based line numbers
- All line indexing in the codebase must be 0-based for consistency
- This includes `line_map` in `CommentBuffer` and `ActionsBuffer` and all navigation calculations
- **Rule**: Use 0-based indexing throughout; only convert to 1-based when displaying to user

### Navigation Sync with Chain Header
- The chain header adds extra lines at the top of the buffer
- `CommentBuffer::line_for_thread()` and `ActionsBuffer::line_for_suite()` account for header offset
- Navigator's internal line_map was ignoring this, causing cursor desync
- **Fix**: Use `buffer.line_for_thread()` / `buffer.line_for_suite()` instead of Navigator's line_map for cursor positioning

### GitHub GraphQL API
- All comment fetching uses a single GraphQL query for performance
- Workflow status uses a separate GraphQL query for check suites/runs
- GraphQL returns camelCase field names; use `#[serde(rename = "camelCaseName")]`
- Review threads include `isResolved` status and all nested comments
- PR chain detection uses separate GraphQL queries for parent/child PRs

### Async State Management
- `AsyncHandle` is used to communicate between background threads and main Neovim thread
- nvim-oxi APIs cannot be called from spawned threads - Lua state is thread-local
- Background threads send results through channels, then call `handle.send()`
- The callback on main thread reads channel and updates UI

### Buffer Non-modifiable
- Sidebar buffer should be `modifiable=false` to prevent accidental edits
- `set_lines()` must temporarily toggle `modifiable=true` during updates

### Sidebar Modes
- The plugin supports two modes: Comments and Actions
- `SidebarMode` enum in `src/types/mode.rs` tracks current mode
- Both modes share the same keymaps but have different behaviors
- PR chain navigation works in both modes, fetching appropriate data

### View Centering on Navigation
- Use `normal! zz` after setting cursor to center the view
- This ensures long comment threads and workflow suites with many jobs are fully visible
- Applied in both `next_comment()`, `prev_comment()`, and `switch_mode()` functions

Concluding notes
----------------
This AGENTS.md is a living document. Update it if the repository structure changes (new directories, tests added, CI introduced), or if the team adopts new tooling (unit test frameworks, CI pipelines). The Commander ensures .opencode/context.md is kept up to date with the project's observed facts.

Reference: plan.md (project plan and architecture)
