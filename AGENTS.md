AGENTS for neogh (Neovim PR Comments Sidebar Plugin)
=================================================

Purpose
-------
This document defines the agent roles, responsibilities, and workflows to be used when working on the neogh repository. It is tailored to the project's structure (Rust Neovim plugin using nvim-oxi) and the team's Autonomous Agent model (Commander, Planner, Worker, Reviewer).

High-level project summary
--------------------------
- Plugin name: neogh — a Neovim PR comments sidebar plugin
- Language: Rust
- Entry point: src/lib.rs (exports commands :PRComments and :PRCommentsClose to Lua)
- Key modules: src/github/* (gh CLI integration and comment fetching), src/ui/* (sidebar, buffer, navigation), src/types/* (shared comment types)

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
  - Read: plan.md, Cargo.toml, src/lib.rs, src/github/*, src/ui/*, lua/
  - Produce: .opencode/todo.md (parallel groups: `github/`, `ui/`, `types+lib.rs`)

- Worker groups (parallelizable)
  - G1: github/ (auth.rs, pr.rs, comments.rs) — GH CLI integration and fetching
  - G2: ui/ (sidebar.rs, buffer.rs, navigation.rs) — Neovim UI + buffer rendering
  - G3: types/ + lib.rs — shared types, exports and lua binding glue

- Reviewer -> Verify build, diagnostics, and runtime behavior after Workers finish each leaf task

Recommended commands and checks (for Workers & Reviewers)
--------------------------------------------------------
- Build: cargo build
- Release build (produce compiled artifact): cargo build --release
- Type / static checks: cargo check
- Tests: cargo test
- Lint/format (optional): cargo clippy, cargo fmt -- --check
- Inspect crate-type & FFI: check Cargo.toml for `crate-type = ["cdylib"|"dylib"]` and inspect target/ directory for produced artifacts to confirm expected lua/neogh.so
- Runtime verification (manual): Open Neovim in the repository, ensure the compiled library is loadable (per project's README), and test :PRComments / :PRCommentsClose and navigation keys (j/k, <CR>, q)

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

Delegation examples (Commander → Planner/Worker/Reviewer)
---------------------------------------------------------
- Create a planning task (Planner):
  delegate_task({ agent: "Planner", background: false, description: "Plan PR detection and GH integration", prompt: "Read plan.md and Cargo.toml; create .opencode/todo.md with tasks for implementing gh CLI auth, pr detection, and comments fetching. Include verification steps (cargo build, lsp diagnostics)" })

- Assign Worker to implement PR detection (Worker):
  delegate_task({ agent: "Worker", background: true, description: "Implement PR detection", prompt: "Edit src/github/pr.rs to run `gh pr view --json number,headRefName`, parse results, return structured PR info. Update .opencode/work-log.md at start and completion. Run cargo build and lsp_diagnostics and attach outputs." })

- Request Reviewer verification after Worker finishes:
  delegate_task({ agent: "Reviewer", background: false, description: "Verify PR detection implementation", prompt: "Run cargo build && cargo test; run lsp_diagnostics on src/github/pr.rs; attempt `gh pr view` in an example repo (or mock) and verify returned structure; update .opencode/sync-issues.md if any mismatch." })

Parallelism guidance
--------------------
- Independent groups (github/, ui/, types/lib) can be worked on concurrently.
- When a Worker changes public types (src/types/*) or lib.rs exports, notify dependent Workers and Reviewer because these are integration boundary changes.

Safety & commit rules
---------------------
- Workers should NOT commit or push changes unless explicitly requested by the user.
- When a commit is requested, follow the Git Safety Protocol: do not amend commits or force push without explicit user approval.

Concluding notes
----------------
This AGENTS.md is a living document. Update it if the repository structure changes (new directories, tests added, CI introduced), or if the team adopts new tooling (unit test frameworks, CI pipelines). The Commander ensures .opencode/context.md is kept up to date with the project's observed facts.

Reference: plan.md (project plan and architecture)
