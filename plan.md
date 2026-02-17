# Neogh - Neovim PR Comments Sidebar Plugin

A Neovim plugin written in Rust using nvim-oxi that displays PR comments in a sidebar with easy navigation.

## Architecture Overview

```
┌─────────────────────────────────────────────────────────────┐
│                    neogh (Plugin Name)                       │
├─────────────────────────────────────────────────────────────┤
│  lib.rs (entry point)                                       │
│  ├── Commands: :PRComments, :PRCommentsClose                │
│  └── Module exports to Lua                                  │
├─────────────────────────────────────────────────────────────┤
│  github/                                                    │
│  ├── mod.rs        - GitHub API module                      │
│  ├── auth.rs       - gh CLI auth integration                │
│  ├── pr.rs         - PR detection via gh CLI                │
│  └── comments.rs   - Fetch review + issue comments          │
├─────────────────────────────────────────────────────────────┤
│  ui/                                                        │
│  ├── mod.rs        - UI module                              │
│  ├── sidebar.rs    - Sidebar window management              │
│  ├── buffer.rs     - Comment buffer rendering               │
│  └── navigation.rs - Cursor tracking & file jumping         │
├─────────────────────────────────────────────────────────────┤
│  types/                                                     │
│  ├── mod.rs        - Shared types                           │
│  └── comment.rs    - Comment data structures                │
└─────────────────────────────────────────────────────────────┘
```

## Dependencies

| Crate | Purpose |
|-------|---------|
| `nvim-oxi` | Neovim FFI bindings |
| `tokio` | Async runtime |
| `serde` | JSON deserialization |
| `chrono` | Timestamp formatting |

## Implementation Details

### PR Detection
- Uses `gh pr view --json number,headRefName` to detect current PR from git branch
- Falls back to error if no PR associated with branch

### Authentication
- Uses `gh auth token` to get GitHub authentication token
- Requires gh CLI to be installed and authenticated

### Comment Types
- **Review Comments**: Line-specific code review comments (include file path and line number)
- **Issue Comments**: General PR discussion comments (no file/line association)

### Sidebar Features
- Vertical split on the right side (~40 columns)
- Comment rendering with syntax highlighting
- File/line indicators for review comments
- Author and timestamp display

### Navigation
- `j/k` to move between comments
- Main window automatically jumps to file:line for review comments
- `<CR>` to explicitly jump and focus main window
- `q` to close sidebar

## Commands

- `:PRComments` - Open the PR comments sidebar
- `:PRCommentsClose` - Close the sidebar

## Project Structure

```
neogh/
├── .cargo/
│   └── config.toml          # macOS linker flags
├── Cargo.toml
├── src/
│   ├── lib.rs
│   ├── github/
│   │   ├── mod.rs
│   │   ├── auth.rs
│   │   ├── pr.rs
│   │   └── comments.rs
│   ├── ui/
│   │   ├── mod.rs
│   │   ├── sidebar.rs
│   │   ├── buffer.rs
│   │   └── navigation.rs
│   └── types/
│       ├── mod.rs
│       └── comment.rs
├── lua/
│   └── neogh.so             # Compiled library
└── README.md
```

## User Workflow

1. User opens a file in a git repo with an active PR
2. Run `:PRComments`
3. Plugin detects PR via `gh pr view`
4. Fetches all comments asynchronously
5. Opens sidebar with rendered comments
6. User navigates with `j/k`; main window jumps to comment location
7. Press `q` or `:PRCommentsClose` to close
