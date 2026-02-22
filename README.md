# neogh

A Neovim plugin for viewing PR comments and GitHub Actions workflow status in a sidebar with easy navigation and stacked PR support.

## Requirements

- Neovim 0.11+
- gh CLI installed and authenticated (run `gh auth login`)
- Working in a git repository with a PR associated with the current branch

## Installation

### lazy.nvim

```lua
{
  "Pajn/neogh",
  build = "cargo build --release",
  config = function()
    -- Optional: set up keymaps
    vim.keymap.set("n", "<leader>pc", function() require("neogh").toggle() end, { desc = "Toggle PR comments" })
  end,
}
```

## Usage

### Commands

- `:PRComments` or `require("neogh").open()` - Open the PR comments sidebar
- `:PRActions` - Open the workflow status sidebar
- `:PRCommentsClose` or `require("neogh").close()` - Close the sidebar
- `require("neogh").toggle()` - Toggle the sidebar

### Modes

The sidebar has two modes:

1. **Comments Mode** - View PR review comments and issue comments
2. **Actions Mode** - View GitHub Actions workflow status and check runs

## Sidebar Keymaps

Active when sidebar is focused:

| Key | Action |
|-----|--------|
| `j` | Next item (comment/workflow) |
| `k` | Previous item (comment/workflow) |
| `<CR>` (Enter) | Comments: jump to file location. Actions: open workflow in browser |
| `<Tab>` | Switch between Comments and Actions mode |
| `za` | Toggle collapse/expand thread (Comments only) |
| `r` | Toggle resolve/unresolve thread (Comments only) |
| `R` | Refresh from GitHub |
| `[p` | Navigate to parent PR in chain |
| `]p` | Navigate to child PR in chain |
| `q` | Close sidebar |

## Features

### PR Comments
- Auto-detects current PR from git branch
- Shows both review comments (line-specific) and issue comments (general)
- Auto-jump to file location when navigating between review comments
- Relative timestamps (e.g., "2 hours ago")
- Thread grouping with expand/collapse
- Resolve/unresolve threads directly from the sidebar

### GitHub Actions Status
- View check suites and individual check runs for the PR's latest commit
- Status icons: ✅ Success, ❌ Failure, 🔄 Running, ⏳ Pending
- Open workflow details in browser with `<CR>`
- See which jobs passed/failed at a glance

### Stacked PR Support
- Automatically detects PR chains (main ← A ← B ← C)
- Navigate between PRs in the chain with `[p` and `]p`
- Shows chain info in the sidebar header
- Background prefetching for instant navigation between PRs in the chain
- Works in both Comments and Actions modes

### Performance
- Single GraphQL query for all comment/workflow data
- Background caching for PR chain navigation
- Non-blocking async loading with instant sidebar open

## Building from Source

Requires Rust toolchain.

```bash
cargo build --release
```

Copy `target/release/libneogh.dylib` (macOS) or `target/release/libneogh.so` (Linux) to `lua/neogh.so`.
