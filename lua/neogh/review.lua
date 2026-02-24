local M = {}

local function notify(msg, level)
  vim.notify(msg, level or vim.log.levels.INFO, { title = "neogh" })
end

local function trim(s)
  return (s or ""):gsub("^%s+", ""):gsub("%s+$", "")
end

local function interpolate(value, pr)
  if type(value) ~= "string" then
    return value
  end
  return value
    :gsub("{number}", tostring(pr.number or ""))
    :gsub("{title}", pr.title or "")
    :gsub("{head_ref}", pr.head_ref or "")
    :gsub("{base_ref}", pr.base_ref or "")
    :gsub("{url}", pr.url or "")
end

local function resolve_checkout_hook(pr)
  local hook = vim.g.neogh_pr_review_checkout_cmd
  if hook == nil or hook == "" then
    return nil
  end

  if type(hook) == "function" then
    local ok, resolved = pcall(hook, pr)
    if not ok then
      notify("Checkout hook function failed: " .. tostring(resolved), vim.log.levels.ERROR)
      return false
    end
    hook = resolved
  end

  if hook == nil or hook == "" then
    return nil
  end

  if type(hook) == "string" then
    return { "sh", "-c", interpolate(hook, pr) }
  end

  if type(hook) == "table" then
    local cmd = {}
    for _, arg in ipairs(hook) do
      cmd[#cmd + 1] = tostring(interpolate(arg, pr))
    end
    return cmd
  end

  notify("`vim.g.neogh_pr_review_checkout_cmd` must be string, table, or function", vim.log.levels.ERROR)
  return false
end

local function run_hook_with_progress(cmd, on_done)
  local spinner = { "|", "/", "-", "\\" }
  local tick = 1
  local notif
  local timer = vim.uv.new_timer()

  local function update_progress()
    local frame = spinner[tick]
    tick = (tick % #spinner) + 1
    notif = vim.notify("Running checkout hook " .. frame, vim.log.levels.INFO, {
      title = "neogh",
      replace = notif,
    })
  end

  update_progress()
  timer:start(120, 120, vim.schedule_wrap(update_progress))

  vim.system(cmd, { text = true }, function(result)
    vim.schedule(function()
      timer:stop()
      timer:close()

      if result.code ~= 0 then
        local err = trim(result.stderr)
        if err == "" then
          err = "checkout hook failed"
        end
        vim.notify("Checkout hook failed: " .. err, vim.log.levels.ERROR, {
          title = "neogh",
          replace = notif,
        })
        on_done(false)
        return
      end

      vim.notify("Checkout hook finished", vim.log.levels.INFO, {
        title = "neogh",
        replace = notif,
      })
      on_done(true)
    end)
  end)
end

local function gh_list_open_prs(on_done)
  vim.system({
    "gh",
    "pr",
    "list",
    "--state",
    "open",
    "--limit",
    "100",
    "--json",
    "number,title,body,headRefName,baseRefName,url",
  }, { text = true }, function(result)
    vim.schedule(function()
      if result.code ~= 0 then
        local err = trim(result.stderr)
        if err == "" then
          err = "failed to query pull requests"
        end
        notify("Failed to query open PRs: " .. err, vim.log.levels.ERROR)
        return
      end

      local ok, prs = pcall(vim.json.decode, result.stdout or "[]")
      if not ok or type(prs) ~= "table" then
        notify("Failed to decode PR list from gh CLI output", vim.log.levels.ERROR)
        return
      end

      on_done(prs)
    end)
  end)
end

local function checkout_pr_and_open_diff(pr)
  vim.system({ "gh", "pr", "checkout", tostring(pr.number) }, { text = true }, function(result)
    vim.schedule(function()
      if result.code ~= 0 then
        local err = trim(result.stderr)
        if err == "" then
          err = "checkout failed"
        end
        notify(("Failed to checkout PR #%d: %s"):format(pr.number, err), vim.log.levels.ERROR)
        return
      end

      local function open_diffview()
        local function load_review_signs()
          local ok_neogh, neogh = pcall(require, "neogh")
          if ok_neogh and type(neogh.load_review_signs) == "function" then
            pcall(neogh.load_review_signs)
          end
        end

        if vim.fn.exists(":DiffviewOpen") == 0 then
          notify(("Checked out PR #%d, but :DiffviewOpen is unavailable"):format(pr.number), vim.log.levels.WARN)
          return
        end

        local base = trim(pr.base_ref)
        if base ~= "" then
          local candidates = {
            "origin/" .. base .. "...HEAD",
            base .. "...HEAD",
            "origin/" .. base .. "..HEAD",
            base .. "..HEAD",
          }

          local seen = {}
          for _, spec in ipairs(candidates) do
            if not seen[spec] then
              seen[spec] = true
              local ok = pcall(vim.cmd, { cmd = "DiffviewOpen", args = { spec } })
              if ok then
                load_review_signs()
                return
              end
            end
          end
        end

        vim.cmd("DiffviewOpen")
        load_review_signs()
      end

      local hook_cmd = resolve_checkout_hook(pr)
      if hook_cmd == false then
        return
      end
      if not hook_cmd then
        open_diffview()
        return
      end

      run_hook_with_progress(hook_cmd, function(ok)
        if ok then
          open_diffview()
        end
      end)
    end)
  end)
end

function M.open()
  local ok, snacks = pcall(require, "snacks")
  if not ok or not snacks.picker then
    notify("snacks.nvim picker is required for PR review selection", vim.log.levels.ERROR)
    return
  end

  gh_list_open_prs(function(prs)
    if vim.tbl_isempty(prs) then
      notify("No open PRs found")
      return
    end

    local items = {}
    for _, pr in ipairs(prs) do
      local number = pr.number or 0
      local title = pr.title or ""
      local head_ref = pr.headRefName or ""
      local base_ref = pr.baseRefName or ""
      local body = trim(pr.body)
      if body == "" then
        body = "_No description provided._"
      end

      items[#items + 1] = {
        number = number,
        title = title,
        head_ref = head_ref,
        base_ref = base_ref,
        url = pr.url,
        preview = {
          text = ("# PR #%d: %s\n\n%s"):format(number, title, body),
          ft = "markdown",
        },
      }
    end

    snacks.picker.select(items, {
      prompt = "Review pull request:",
      format_item = function(item)
        return ("#%d %s [%s -> %s]"):format(item.number, item.title, item.base_ref, item.head_ref)
      end,
      snacks = {
        title = "Open Pull Requests",
        preview = "preview",
        layout = {
          layout = {
            backdrop = false,
            width = 100,
            min_width = 80,
            max_width = 120,
            height = 24,
            min_height = 10,
            box = "vertical",
            border = true,
            title = "{title}",
            title_pos = "center",
            { win = "input", height = 1, border = "bottom" },
            { win = "list", border = "none", height = math.max(math.min(#items, 10), 2) },
            { win = "preview", title = "{preview}", border = "top" },
          },
        },
      },
    }, function(item)
      if not item then
        return
      end
      checkout_pr_and_open_diff(item)
    end)
  end)
end

return M
