local cli = require("tt.cli")
local buffer = require("tt.buffer")
local M = {}

function M.setup(options)
  cli.setup((options or {}).bin)
end

local function start_dir()
  local current = buffer.project(vim.api.nvim_get_current_buf())
  if current then
    return current.path
  end
  local name = vim.api.nvim_buf_get_name(0)
  if vim.bo.buftype == "" and name ~= "" then
    return vim.fn.fnamemodify(name, ":p:h")
  end
  return vim.fn.getcwd()
end

local function resolve()
  local current, err = cli.run(start_dir(), { "project", "current" })
  if not current then
    vim.notify("tt: " .. err.error, vim.log.levels.ERROR)
    return nil
  end
  if current.registered then
    return current
  end
  if vim.fn.confirm("Register project " .. current.path .. "?", "&Register\n&Cancel", 2) ~= 1 then
    return nil
  end
  local project
  project, err = cli.run(current.path, { "project", "add", current.path })
  if not project then
    vim.notify("tt: " .. err.error, vim.log.levels.ERROR)
  end
  return project
end

function M.command(args)
  if args == "projects" then
    local result, err = cli.run(nil, { "project", "list" })
    if not result then
      vim.notify("tt: " .. err.error, vim.log.levels.ERROR)
      return
    end
    vim.ui.select(result.projects, {
      prompt = "TT projects",
      format_item = function(project)
        return project.slug .. "  " .. project.path
      end,
    }, function(project)
      if project then
        buffer.open(project)
      end
    end)
    return
  end
  local title = args:match("^add%s+(.+)$")
  if args ~= "" and not title then
    vim.notify("tt: use :TT, :TT add <title>, or :TT projects", vim.log.levels.ERROR)
    return
  end
  local project = resolve()
  if not project then
    return
  end
  if title then
    local result, err = cli.run(project.path, { "add", "--title=" .. title })
    if not result then
      vim.notify("tt: " .. err.error, vim.log.levels.ERROR)
      return
    end
    buffer.refresh(project.path)
  else
    buffer.open(project)
  end
end

return M
