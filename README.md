# neogh

A Neovim plugin for viewing PR comments in a sidebar with easy navigation and stacked PR support.

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

- `:PRComments` or `require("neogh").open()` - Open the PR comments sidebar
- `:PRCommentsClose` or `require("neogh").close()` - Close the sidebar
- `require("neogh").toggle()` - Toggle the sidebar

## Sidebar Keymaps

Active when sidebar is focused:

| Key | Action |
|-----|--------|
| `j` | Next comment |
| `k` | Previous comment |
| `<CR>` (Enter) | Jump to comment location in main window |
| `za` | Toggle collapse/expand thread |
| `r` | Toggle resolve/unresolve thread |
| `R` | Refresh comments from GitHub |
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

### Stacked PR Support
- Automatically detects PR chains (main ← A ← B ← C)
- Navigate between PRs in the chain with `[p` and `]p`
- Shows chain info in the sidebar header
- Background prefetching for instant navigation between PRs in the chain

### Performance
- Single GraphQL query for all comment data
- Background caching for PR chain navigation
- Non-blocking async loading with instant sidebar open

## Building from Source

Requires Rust toolchain.

```bash
cargo build --release
```

Copy `target/release/libneogh.dylib` (macOS) or `target/release/libneogh.so` (Linux) to `lua/neogh.so`.
