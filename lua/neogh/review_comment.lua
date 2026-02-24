local M = {}

local sessions = {}

local function notify(msg, level)
  vim.notify(msg, level or vim.log.levels.INFO, { title = "neogh" })
end

local function trim(s)
  return (s or ""):gsub("^%s+", ""):gsub("%s+$", "")
end

local function clamp_line(line, max_line)
  line = tonumber(line) or 1
  if line < 1 then
    return 1
  end
  if line > max_line then
    return max_line
  end
  return line
end

local function get_selection_range(bufnr)
  local max_line = vim.api.nvim_buf_line_count(bufnr)
  local mark_start = vim.fn.getpos("'<")
  local mark_end = vim.fn.getpos("'>")

  local same_buf_start = mark_start[1] == 0 or mark_start[1] == bufnr
  local same_buf_end = mark_end[1] == 0 or mark_end[1] == bufnr

  if mark_start[2] > 0 and mark_end[2] > 0 and same_buf_start and same_buf_end then
    local from = clamp_line(mark_start[2], max_line)
    local to = clamp_line(mark_end[2], max_line)
    if from > to then
      from, to = to, from
    end
    return from, to
  end

  local line = clamp_line(vim.api.nvim_win_get_cursor(0)[1], max_line)
  return line, line
end

local function in_diffview_context()
  local ok, lib = pcall(require, "diffview.lib")
  if not ok then
    return nil, "diffview.nvim is required"
  end

  local view = lib.get_current_view()
  if not view or not view.cur_entry or not view.cur_layout then
    return nil, "Run this command from an active Diffview file window"
  end

  local winid = vim.api.nvim_get_current_win()
  local symbol
  local windows = view.cur_layout.windows or {}
  for _, win in ipairs(windows) do
    if win.id == winid then
      symbol = win.file and win.file.symbol or nil
      break
    end
  end

  if symbol and symbol ~= "b" then
    return nil, "Select lines in the right (working tree) Diffview pane"
  end

  local bufnr = vim.api.nvim_get_current_buf()
  local cursor = vim.api.nvim_win_get_cursor(0)
  local start_line, end_line = get_selection_range(bufnr)
  local selected_lines = vim.api.nvim_buf_get_lines(bufnr, start_line - 1, end_line, false)

  local cwd = view.adapter and view.adapter.ctx and view.adapter.ctx.toplevel or vim.fn.getcwd()
  local path = tostring(view.cur_entry.path or "")
  if path == "" then
    return nil, "Could not resolve file path from Diffview"
  end

  local cwd_prefix = cwd .. "/"
  if path:sub(1, #cwd_prefix) == cwd_prefix then
    path = path:sub(#cwd_prefix + 1)
  end

  return {
    cwd = cwd,
    path = path,
    start_line = start_line,
    end_line = end_line,
    selected_lines = selected_lines,
    origin_win = vim.api.nvim_get_current_win(),
    origin_buf = bufnr,
    origin_line = cursor[1],
    origin_col = cursor[2],
  }
end

local function parse_hunk_ranges(diff_text)
  local ranges = {}
  for _, line in ipairs(vim.split(diff_text or "", "\n", { plain = true })) do
    local start_new, count_new = line:match("^@@ %-%d+,?%d* %+(%d+),?(%d*) @@")
    if start_new then
      local start_line = tonumber(start_new) or 0
      local count = count_new == "" and 1 or (tonumber(count_new) or 0)
      if count > 0 then
        ranges[#ranges + 1] = {
          start_line = start_line,
          end_line = start_line + count - 1,
        }
      end
    end
  end
  return ranges
end

local function has_overlap(ranges, from_line, to_line)
  for _, range in ipairs(ranges) do
    if from_line <= range.end_line and to_line >= range.start_line then
      return true
    end
  end
  return false
end

local function check_unstaged_overlap(context, on_done)
  vim.system({ "git", "diff", "--unified=0", "--", context.path }, {
    cwd = context.cwd,
    text = true,
  }, function(result)
    vim.schedule(function()
      if result.code ~= 0 then
        on_done(false)
        return
      end

      local ranges = parse_hunk_ranges(result.stdout)
      on_done(has_overlap(ranges, context.start_line, context.end_line))
    end)
  end)
end

local function default_editor_lines(context, include_suggestion)
  local lines = { "" }

  if include_suggestion then
    lines[#lines + 1] = "```suggestion"
    for _, line in ipairs(context.selected_lines) do
      lines[#lines + 1] = line
    end
    lines[#lines + 1] = "```"
    lines[#lines + 1] = ""
  end

  return lines
end

local function run_gh_json(args, cwd, on_done)
  local cmd = { "gh" }
  for _, arg in ipairs(args) do
    cmd[#cmd + 1] = arg
  end

  vim.system(cmd, { cwd = cwd, text = true }, function(result)
    vim.schedule(function()
      if result.code ~= 0 then
        local err = trim(result.stderr)
        if err == "" then
          err = "gh command failed"
        end
        on_done(nil, err)
        return
      end

      local ok, data = pcall(vim.json.decode, result.stdout or "{}")
      if not ok then
        on_done(nil, "Failed to decode gh output")
        return
      end

      on_done(data, nil)
    end)
  end)
end

local function gh_graphql(query, vars, cwd, on_done)
  local args = { "api", "graphql", "-f", "query=" .. query }
  for key, value in pairs(vars or {}) do
    args[#args + 1] = "-F"
    args[#args + 1] = key .. "=" .. tostring(value)
  end

  run_gh_json(args, cwd, function(payload, err)
    if err then
      on_done(nil, err)
      return
    end

    if type(payload.errors) == "table" and #payload.errors > 0 then
      local errors = {}
      for _, item in ipairs(payload.errors) do
        errors[#errors + 1] = tostring(item.message or item)
      end
      on_done(nil, table.concat(errors, "; "))
      return
    end

    on_done(payload.data, nil)
  end)
end

local function get_current_pr_info(cwd, on_done)
  run_gh_json({ "pr", "view", "--json", "number,url" }, cwd, function(data, err)
    if err then
      on_done(nil, "Failed to inspect current PR: " .. err)
      return
    end

    local url = tostring(data.url or "")
    local owner, repo, number = url:match("^https?://[^/]+/([^/]+)/([^/]+)/pull/(%d+)")
    if not owner or not repo or not number then
      on_done(nil, "Could not parse owner/repo from PR URL: " .. url)
      return
    end

    on_done({
      cwd = cwd,
      owner = owner,
      repo = repo,
      number = tonumber(number),
    }, nil)
  end)
end

local function ensure_pending_review(pr_info, on_done)
  local query = [[
query($owner: String!, $repo: String!, $number: Int!) {
  viewer {
    login
  }
  repository(owner: $owner, name: $repo) {
    pullRequest(number: $number) {
      id
      headRefOid
      reviews(last: 50) {
        nodes {
          id
          state
          author {
            login
          }
        }
      }
    }
  }
}
]]

  gh_graphql(query, {
    owner = pr_info.owner,
    repo = pr_info.repo,
    number = pr_info.number,
  }, pr_info.cwd, function(data, err)
    if err then
      on_done(nil, err)
      return
    end

    local pr = data
      and data.repository
      and data.repository.pullRequest
      or nil
    if not pr or not pr.id then
      on_done(nil, "Failed to load pull request metadata")
      return
    end

    local viewer_login = data.viewer and data.viewer.login or nil
    local nodes = pr.reviews and pr.reviews.nodes or {}
    for _, review in ipairs(nodes) do
      local author = review.author and review.author.login or nil
      if review.id and review.state == "PENDING" and author == viewer_login then
        on_done({
          pull_request_id = pr.id,
          pending_review_id = review.id,
        }, nil)
        return
      end
    end

    local create_mutation = [[
mutation($pullRequestId: ID!, $commitOID: GitObjectID!) {
  addPullRequestReview(input: { pullRequestId: $pullRequestId, commitOID: $commitOID }) {
    pullRequestReview {
      id
    }
  }
}
]]

    gh_graphql(create_mutation, {
      pullRequestId = pr.id,
      commitOID = pr.headRefOid,
    }, pr_info.cwd, function(create_data, create_err)
      if create_err then
        on_done(nil, create_err)
        return
      end

      local review_id = create_data
        and create_data.addPullRequestReview
        and create_data.addPullRequestReview.pullRequestReview
        and create_data.addPullRequestReview.pullRequestReview.id
        or nil

      if not review_id then
        on_done(nil, "Failed to create pending review")
        return
      end

      on_done({
        pull_request_id = pr.id,
        pending_review_id = review_id,
      }, nil)
    end)
  end)
end

local function submit_review_thread(pr_info, review_ctx, context, body, on_done)
  local single_line_mutation = [[
mutation(
  $pullRequestId: ID!,
  $pullRequestReviewId: ID!,
  $path: String!,
  $body: String!,
  $line: Int!
) {
  addPullRequestReviewThread(
    input: {
      pullRequestId: $pullRequestId,
      pullRequestReviewId: $pullRequestReviewId,
      path: $path,
      body: $body,
      line: $line,
      side: RIGHT
    }
  ) {
    thread {
      id
    }
  }
}
]]

  local multi_line_mutation = [[
mutation(
  $pullRequestId: ID!,
  $pullRequestReviewId: ID!,
  $path: String!,
  $body: String!,
  $line: Int!,
  $startLine: Int!
) {
  addPullRequestReviewThread(
    input: {
      pullRequestId: $pullRequestId,
      pullRequestReviewId: $pullRequestReviewId,
      path: $path,
      body: $body,
      line: $line,
      side: RIGHT,
      startLine: $startLine,
      startSide: RIGHT
    }
  ) {
    thread {
      id
    }
  }
}
]]

  local vars = {
    pullRequestId = review_ctx.pull_request_id,
    pullRequestReviewId = review_ctx.pending_review_id,
    path = context.path,
    body = body,
    line = context.end_line,
  }

  local mutation = single_line_mutation
  if context.start_line < context.end_line then
    vars.startLine = context.start_line
    mutation = multi_line_mutation
  end

  gh_graphql(mutation, vars, pr_info.cwd, function(_, err)
    if err then
      on_done(err)
      return
    end
    on_done(nil)
  end)
end

local function close_session_buffer(bufnr)
  local session = sessions[bufnr]
  sessions[bufnr] = nil

  local wins = vim.fn.win_findbuf(bufnr)
  for _, win in ipairs(wins) do
    if vim.api.nvim_win_is_valid(win) then
      vim.api.nvim_win_close(win, true)
    end
  end

  if vim.api.nvim_buf_is_valid(bufnr) then
    pcall(vim.api.nvim_buf_delete, bufnr, { force = true })
  end

  if session and session.context then
    local ctx = session.context
    if ctx.origin_win and vim.api.nvim_win_is_valid(ctx.origin_win) then
      pcall(vim.api.nvim_set_current_win, ctx.origin_win)
      if ctx.origin_buf and vim.api.nvim_win_get_buf(ctx.origin_win) == ctx.origin_buf then
        pcall(vim.api.nvim_win_set_cursor, ctx.origin_win, { ctx.origin_line or ctx.start_line, ctx.origin_col or 0 })
        pcall(vim.cmd, "normal! zz")
      end
    end
  end
end

local function mark_comment_range(context)
  local ok, neogh = pcall(require, "neogh")
  if ok and type(neogh.load_review_signs) == "function" then
    pcall(neogh.load_review_signs)
  end
end

local function open_editor(context, include_suggestion)
  local height = tonumber(vim.g.neogh_pr_comment_split_height) or 10
  vim.cmd(("botright %dsplit"):format(height))

  local winid = vim.api.nvim_get_current_win()
  local bufnr = vim.api.nvim_create_buf(false, true)
  vim.api.nvim_win_set_buf(winid, bufnr)
  vim.api.nvim_buf_set_name(bufnr, ("neogh://review-comment/%s:%d-%d"):format(
    context.path,
    context.start_line,
    context.end_line
  ))

  vim.bo[bufnr].buftype = "nofile"
  vim.bo[bufnr].bufhidden = "wipe"
  vim.bo[bufnr].swapfile = false
  vim.bo[bufnr].modifiable = true
  vim.bo[bufnr].filetype = "markdown"

  vim.api.nvim_buf_set_lines(bufnr, 0, -1, false, default_editor_lines(context, include_suggestion))

  sessions[bufnr] = {
    context = context,
    submitting = false,
  }

  vim.keymap.set("n", "q", function()
    close_session_buffer(bufnr)
  end, { buffer = bufnr, silent = true, nowait = true })

  vim.keymap.set({ "n", "i" }, "<C-s>", function()
    M.submit(bufnr)
  end, { buffer = bufnr, silent = true, nowait = true })

  notify(("Review comment editor opened for %s:%d-%d"):format(
    context.path,
    context.start_line,
    context.end_line
  ))
  notify("Write your comment and press <C-s> to submit (q to cancel)")
end

function M.open()
  local context, err = in_diffview_context()
  if not context then
    notify(err, vim.log.levels.ERROR)
    return
  end

  check_unstaged_overlap(context, function(include_suggestion)
    open_editor(context, include_suggestion)
  end)
end

function M.submit(bufnr)
  bufnr = bufnr or vim.api.nvim_get_current_buf()
  local session = sessions[bufnr]
  if not session then
    notify("No active review comment session for this buffer", vim.log.levels.ERROR)
    return
  end
  if session.submitting then
    notify("Review comment submission already in progress")
    return
  end

  local lines = vim.api.nvim_buf_get_lines(bufnr, 0, -1, false)
  local body = table.concat(lines, "\n")
  if trim(body) == "" then
    notify("Review comment body cannot be empty", vim.log.levels.ERROR)
    return
  end

  session.submitting = true
  local context = session.context
  local progress = vim.notify("Submitting review comment...", vim.log.levels.INFO, { title = "neogh" })

  get_current_pr_info(context.cwd, function(pr_info, pr_err)
    if pr_err then
      local s = sessions[bufnr]
      if s then
        s.submitting = false
      end
      vim.notify(pr_err, vim.log.levels.ERROR, { title = "neogh", replace = progress })
      return
    end

    ensure_pending_review(pr_info, function(review_ctx, review_err)
      if review_err then
        local s = sessions[bufnr]
        if s then
          s.submitting = false
        end
        vim.notify("Failed to prepare pending review: " .. review_err, vim.log.levels.ERROR, {
          title = "neogh",
          replace = progress,
        })
        return
      end

      submit_review_thread(pr_info, review_ctx, context, body, function(submit_err)
        if submit_err then
          local s = sessions[bufnr]
          if s then
            s.submitting = false
          end
          vim.notify("Failed to add review comment: " .. submit_err, vim.log.levels.ERROR, {
            title = "neogh",
            replace = progress,
          })
          return
        end

        vim.notify("Review comment added to pending review", vim.log.levels.INFO, {
          title = "neogh",
          replace = progress,
        })
        mark_comment_range(context)
        local s = sessions[bufnr]
        if s then
          s.submitting = false
        end
        close_session_buffer(bufnr)
      end)
    end)
  end)
end

return M
