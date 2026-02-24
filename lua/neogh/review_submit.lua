local M = {}

local sessions = {}

local EVENT_LABELS = {
  APPROVE = "Approve",
  REQUEST_CHANGES = "Request Changes",
  COMMENT = "Comment",
}

local function notify(msg, level, opts)
  opts = opts or {}
  vim.notify(msg, level or vim.log.levels.INFO, vim.tbl_extend("force", { title = "neogh" }, opts))
end

local function trim(s)
  return (s or ""):gsub("^%s+", ""):gsub("%s+$", "")
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

local function infer_cwd()
  local ok, lib = pcall(require, "diffview.lib")
  if ok then
    local view = lib.get_current_view()
    if view and view.adapter and view.adapter.ctx and view.adapter.ctx.toplevel then
      return view.adapter.ctx.toplevel
    end
  end
  return vim.fn.getcwd()
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

    local pr = data and data.repository and data.repository.pullRequest or nil
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

local function submit_review(pr_info, review_ctx, event, body, on_done)
  local mutation = [[
mutation(
  $pullRequestId: ID!,
  $pullRequestReviewId: ID!,
  $event: PullRequestReviewEvent!,
  $body: String!
) {
  submitPullRequestReview(
    input: {
      pullRequestId: $pullRequestId,
      pullRequestReviewId: $pullRequestReviewId,
      event: $event,
      body: $body
    }
  ) {
    pullRequestReview {
      id
      state
    }
  }
}
]]

  gh_graphql(mutation, {
    pullRequestId = review_ctx.pull_request_id,
    pullRequestReviewId = review_ctx.pending_review_id,
    event = event,
    body = body,
  }, pr_info.cwd, function(_, err)
    on_done(err)
  end)
end

local function close_session_buffer(bufnr)
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
end

local function open_editor(event)
  local label = EVENT_LABELS[event]
  local height = tonumber(vim.g.neogh_pr_review_finish_split_height) or 8
  vim.cmd(("botright %dsplit"):format(height))

  local winid = vim.api.nvim_get_current_win()
  local bufnr = vim.api.nvim_create_buf(false, true)
  vim.api.nvim_win_set_buf(winid, bufnr)
  vim.api.nvim_buf_set_name(bufnr, ("neogh://review-finish/%s"):format(event:lower()))

  vim.bo[bufnr].buftype = "nofile"
  vim.bo[bufnr].bufhidden = "wipe"
  vim.bo[bufnr].swapfile = false
  vim.bo[bufnr].modifiable = true
  vim.bo[bufnr].filetype = "markdown"

  vim.api.nvim_buf_set_lines(bufnr, 0, -1, false, { "" })

  sessions[bufnr] = {
    event = event,
    label = label,
    cwd = infer_cwd(),
  }

  vim.keymap.set("n", "q", function()
    close_session_buffer(bufnr)
  end, { buffer = bufnr, silent = true, nowait = true })

  vim.keymap.set({ "n", "i" }, "<C-s>", function()
    M.submit(bufnr)
  end, { buffer = bufnr, silent = true, nowait = true })

  notify(("Finish review (%s): write message and press <C-s> (q to cancel)"):format(label))
end

function M.open(event)
  if not EVENT_LABELS[event] then
    notify("Invalid review event: " .. tostring(event), vim.log.levels.ERROR)
    return
  end
  open_editor(event)
end

function M.submit(bufnr)
  bufnr = bufnr or vim.api.nvim_get_current_buf()
  local session = sessions[bufnr]
  if not session then
    notify("No active finish-review session for this buffer", vim.log.levels.ERROR)
    return
  end

  local lines = vim.api.nvim_buf_get_lines(bufnr, 0, -1, false)
  local body = table.concat(lines, "\n")
  if trim(body) == "" then
    notify("Review message cannot be empty", vim.log.levels.ERROR)
    return
  end

  local progress = notify(("Submitting %s review..."):format(session.label), vim.log.levels.INFO)

  get_current_pr_info(session.cwd, function(pr_info, pr_err)
    if pr_err then
      notify(pr_err, vim.log.levels.ERROR, { replace = progress })
      return
    end

    ensure_pending_review(pr_info, function(review_ctx, review_err)
      if review_err then
        notify("Failed to prepare pending review: " .. review_err, vim.log.levels.ERROR, { replace = progress })
        return
      end

      submit_review(pr_info, review_ctx, session.event, body, function(submit_err)
        if submit_err then
          notify("Failed to submit review: " .. submit_err, vim.log.levels.ERROR, { replace = progress })
          return
        end

        notify(("Review submitted: %s"):format(session.label), vim.log.levels.INFO, { replace = progress })
        close_session_buffer(bufnr)
      end)
    end)
  end)
end

return M
