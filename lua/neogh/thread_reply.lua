local M = {}

local sessions = {}

local function notify(msg, level, opts)
  opts = opts or {}
  local merged_opts = vim.tbl_extend("force", { title = "neogh" }, opts)
  local result = vim.notify(msg, level or vim.log.levels.INFO, merged_opts)
  if result ~= nil then
    return result
  end
  return merged_opts.id
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

local function refresh_sidebar()
  local ok, neogh = pcall(require, "neogh")
  if ok and type(neogh.refresh) == "function" then
    pcall(neogh.refresh)
  end
end

function M.open(thread_id)
  if type(thread_id) ~= "string" or thread_id == "" then
    notify("Thread ID is required for reply", vim.log.levels.ERROR)
    return
  end

  local height = tonumber(vim.g.neogh_pr_thread_reply_split_height) or 8
  vim.cmd(("botright %dsplit"):format(height))

  local winid = vim.api.nvim_get_current_win()
  local bufnr = vim.api.nvim_create_buf(false, true)
  vim.api.nvim_win_set_buf(winid, bufnr)
  vim.api.nvim_buf_set_name(bufnr, ("neogh://thread-reply/%s"):format(thread_id))

  vim.bo[bufnr].buftype = "nofile"
  vim.bo[bufnr].bufhidden = "wipe"
  vim.bo[bufnr].swapfile = false
  vim.bo[bufnr].modifiable = true
  vim.bo[bufnr].filetype = "markdown"

  vim.api.nvim_buf_set_lines(bufnr, 0, -1, false, { "" })

  sessions[bufnr] = {
    thread_id = thread_id,
    cwd = vim.fn.getcwd(),
  }

  vim.keymap.set("n", "q", function()
    close_session_buffer(bufnr)
  end, { buffer = bufnr, silent = true, nowait = true })

  vim.keymap.set({ "n", "i" }, "<C-s>", function()
    M.submit(bufnr)
  end, { buffer = bufnr, silent = true, nowait = true })

  notify("Write reply and press <C-s> to submit (q to cancel)")
end

function M.submit(bufnr)
  bufnr = bufnr or vim.api.nvim_get_current_buf()
  local session = sessions[bufnr]
  if not session then
    notify("No active thread reply session for this buffer", vim.log.levels.ERROR)
    return
  end

  local lines = vim.api.nvim_buf_get_lines(bufnr, 0, -1, false)
  local body = table.concat(lines, "\n")
  if trim(body) == "" then
    notify("Reply body cannot be empty", vim.log.levels.ERROR)
    return
  end

  local progress_id = ("neogh-thread-reply-submit-%d"):format(bufnr)
  local progress = notify("Submitting thread reply...", vim.log.levels.INFO, {
    id = progress_id,
    timeout = 10000,
  })
  local mutation = [[
mutation($threadId: ID!, $body: String!) {
  addPullRequestReviewThreadReply(
    input: {
      pullRequestReviewThreadId: $threadId,
      body: $body
    }
  ) {
    comment {
      id
    }
  }
}
]]

  gh_graphql(mutation, {
    threadId = session.thread_id,
    body = body,
  }, session.cwd, function(_, err)
    if err then
      notify("Failed to submit thread reply: " .. err, vim.log.levels.ERROR, {
        id = progress_id,
        replace = progress,
      })
      return
    end

    notify("Thread reply submitted", vim.log.levels.INFO, { id = progress_id, replace = progress })
    close_session_buffer(bufnr)
    refresh_sidebar()
  end)
end

return M
