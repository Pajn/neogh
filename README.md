# neogh

A Neovim plugin for viewing PR comments and GitHub Actions workflow status in a sidebar with easy navigation and stacked PR support.

## Requirements

- Neovim 0.11+
- gh CLI installed and authenticated (run `gh auth login`)
- Working in a git repository with a PR associated with the current branch
- Optional for `:PRReview`: [snacks.nvim](https://github.com/folke/snacks.nvim) and [diffview.nvim](https://github.com/sindrets/diffview.nvim)

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
- `:PRReview` or `require("neogh").review_pr()` - Pick an open PR in Snacks, checkout the branch, and open Diffview against the PR base branch
- `:PRReviewSignsLoad` or `require("neogh").load_review_signs()` - Async load review-comment signs (`RC`) into Diffview buffers
- `:PRReviewComment` or `require("neogh").review_comment()` - In Diffview, open a markdown editor for a line/range review comment
- `:PRReviewApprove` or `require("neogh").review_approve()` - Finish review as approve (with message split)
- `:PRReviewRequestChanges` or `require("neogh").review_request_changes()` - Finish review as request changes (with message split)
- `:PRReviewFinishComment` or `require("neogh").review_finish_comment()` - Finish review as comment (with message split)
- `:PRReplyThread` or `require("neogh").reply_to_thread()` - Reply to selected review thread in comments sidebar
- `:PRPendingComments` or `require("neogh").open_pending_comments()` - Open sidebar with your pending review comments
- `:PRCommentEdit` - Edit selected comment in sidebar (your own comments only)
- `:PRCommentDelete` - Delete selected comment in sidebar (your own comments only)
- `:PRCommentsClose` or `require("neogh").close()` - Close the sidebar
- `require("neogh").toggle()` - Toggle the sidebar

### PRReview Hook Configuration

You can run an additional command after `gh pr checkout` and before `:DiffviewOpen`:

```lua
-- String: executed with `sh -c`
vim.g.neogh_pr_review_checkout_cmd = "npm install"

-- List form: executed directly (no shell)
vim.g.neogh_pr_review_checkout_cmd = { "just", "bootstrap" }

-- Function: receives selected PR item and returns string/list/nil
vim.g.neogh_pr_review_checkout_cmd = function(pr)
  if pr.base_ref == "main" then
    return { "just", "setup-main" }
  end
  return nil
end
```

Placeholders supported in string/list entries: `{number}`, `{title}`, `{head_ref}`, `{base_ref}`, `{url}`.
The hook runs asynchronously and shows a progress notification while it is running.

### Diffview Review Comments

Inside `:DiffviewOpen`:

1. Select a line range (visual mode) in the **right/working-tree pane**
2. Run `:PRReviewComment`
3. Write your comment in the small horizontal markdown split
4. Press `<C-s>` to submit to the pending review (`q` to cancel)

Behavior:
- If your pending review does not exist, neogh creates one automatically
- The visual selection is used as the GitHub review comment range
- If unstaged changes overlap that selected range, a GitHub suggestion code fence is prefilled
- After `:PRReview` opens Diffview, neogh asynchronously loads signs for existing review comments

### Sidebar Thread Replies

In `:PRComments` (Comments mode), move to a review thread and press `c` (or run `:PRReplyThread`).
This opens the same small horizontal markdown editor:
- `<C-s>` submit reply
- `q` cancel

### Finish Review Commands

Use one of:
- `:PRReviewApprove`
- `:PRReviewRequestChanges`
- `:PRReviewFinishComment`

Each opens a small horizontal markdown split for your final message.
Press `<C-s>` to submit the review event, or `q` to cancel.

### Sidebar Comment Editing / Deletion

In sidebars:
- `:PRPendingComments`: you can edit/delete your pending review comments
- `:PRComments`: you can edit/delete your own root comments in the selected thread

- `e` opens a small markdown split to edit the selected comment (`<C-s>` save, `q` cancel)
- `d` deletes the selected comment

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
| `c` | Reply to selected review thread (Comments only) |
| `e` | Edit selected comment (your own comments only) |
| `d` | Delete selected comment (your own comments only) |
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
