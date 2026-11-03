local cli = require("tt.cli")
local format = require("tt.format")
local M = {}
local projects = {}
local namespace = vim.api.nvim_create_namespace("tt-view")

local function notify(message, level)
  vim.notify("tt: " .. message, level or vim.log.levels.ERROR)
end

local function decorate(buf)
  if not vim.api.nvim_buf_is_valid(buf) or not projects[buf] then
    return
  end
  vim.api.nvim_buf_clear_namespace(buf, namespace, 0, -1)
  local project = projects[buf]
  local header = "tt: " .. vim.fn.fnamemodify(project.path, ":~") .. (vim.bo[buf].modified and "  [modified]" or "")
  vim.api.nvim_buf_set_extmark(buf, namespace, 0, 0, {
    virt_lines = { { { header, "Title" } }, {} },
    virt_lines_above = true,
  })
end

local function reload(buf)
  local project = projects[buf]
  local list, err = cli.run(project.path, { "list" })
  if not list then
    notify(err.error)
    return false
  end
  local lines, originals = format.render(list.tasks)
  if not lines then
    notify(originals)
    return false
  end
  if #lines == 0 then
    lines = { "" }
  end
  vim.api.nvim_buf_set_lines(buf, 0, -1, false, lines)
  project.revision, project.originals = list.revision, originals
  vim.bo[buf].modified = false
  decorate(buf)
  return true
end

local function summary(plan)
  local lines = {
    string.format(
      "Adds: %d  Updates: %d  Moves: %d  Reorders: %d",
      #plan.adds,
      #plan.updates,
      #plan.moves,
      plan.reorders
    ),
    string.format("DELETES: %d", #plan.deletes),
  }
  for _, deletion in ipairs(plan.deletes) do
    lines[#lines + 1] = "  - " .. deletion.title .. (deletion.has_body and " [HAS BODY]" or "")
  end
  return table.concat(lines, "\n")
end

local function report_error(err)
  local message = err.error or "unknown error"
  if err.applied then
    message = message .. "\nAlready applied:\n" .. summary(err.applied) .. "\n" .. vim.json.encode(err.applied)
  end
  notify(message)
end

local function write(buf)
  local project = projects[buf]
  local nodes, parse_error = format.parse(vim.api.nvim_buf_get_lines(buf, 0, -1, false), project.originals)
  if not nodes then
    notify(parse_error)
    return
  end
  local input = vim.json.encode({ revision = project.revision, tasks = nodes })
  local plan, err = cli.run(project.path, { "apply", "--dry-run" }, input)
  if not plan then
    report_error(err)
    return
  end
  if #plan.adds + #plan.updates + #plan.moves + plan.reorders + #plan.deletes == 0 then
    vim.bo[buf].modified = false
    decorate(buf)
    return
  end
  if vim.fn.confirm(summary(plan), "&Apply\n&Cancel", 2) ~= 1 then
    return
  end
  local applied
  applied, err = cli.run(project.path, { "apply" }, input)
  if not applied then
    report_error(err)
    return
  end
  reload(buf)
end

local function id_at_cursor()
  return vim.api.nvim_get_current_line():match("^%s*/([a-z0-9_-]+)%s+")
end

local function open_task(buf)
  local id = id_at_cursor()
  if not id then
    notify("save this task before opening it", vim.log.levels.INFO)
    return
  end
  local task, err = cli.run(projects[buf].path, { "show", id })
  if not task then
    report_error(err)
    return
  end
  vim.cmd.edit(vim.fn.fnameescape(task.path))
end

local function toggle()
  local line = vim.api.nvim_get_current_line()
  local count
  line, count = line:gsub("^(%s*/[a-z0-9_-]+%s+%- %[)([ x%-])(%])", function(prefix, marker, suffix)
    return prefix .. (marker == " " and "x" or " ") .. suffix
  end, 1)
  if count == 0 then
    line, count = line:gsub("^(%s*%- %[)([ x%-])(%])", function(prefix, marker, suffix)
      return prefix .. (marker == " " and "x" or " ") .. suffix
    end, 1)
  end
  if count == 0 and (line:match("^%s*/[a-z0-9_-]+%s+%- %[") or line:match("^%s*%- %[")) then
    notify("invalid checkbox")
    return
  end
  if count == 0 and line:match("%S") then
    line, count = line:gsub("^(%s*/[a-z0-9_-]+%s+)", "%1- [x] ", 1)
    if count == 0 then
      line = line:gsub("^(%s*)", "%1- [x] ", 1)
    end
  end
  vim.api.nvim_set_current_line(line)
end

local function clamp()
  local line = vim.api.nvim_get_current_line()
  local prefix = line:match("^(%s*/[a-z0-9_-]+%s+)")
  local cursor = vim.api.nvim_win_get_cursor(0)
  if prefix and cursor[2] < #prefix then
    vim.api.nvim_win_set_cursor(0, { cursor[1], #prefix })
  end
end

local function initialize(buf, project)
  projects[buf] = { path = project.path, slug = project.slug }
  vim.bo[buf].buftype = "acwrite"
  vim.bo[buf].bufhidden = "hide"
  vim.bo[buf].swapfile = false
  vim.bo[buf].filetype = "tt"
  vim.bo[buf].expandtab = true
  vim.bo[buf].shiftwidth = 2
  vim.bo[buf].tabstop = 2
  vim.bo[buf].autoindent = true
  vim.api.nvim_buf_call(buf, function()
    vim.cmd([[syntax match ttIdentity /^\s*\zs\/[0-9a-z_-]\+\s/ conceal]])
  end)
  vim.api.nvim_create_autocmd("BufWriteCmd", {
    buffer = buf,
    callback = function()
      write(buf)
    end,
  })
  vim.api.nvim_create_autocmd("BufReadCmd", {
    buffer = buf,
    callback = function()
      reload(buf)
    end,
  })
  vim.api.nvim_create_autocmd("BufEnter", {
    buffer = buf,
    callback = function()
      if not vim.bo[buf].modified then
        reload(buf)
      end
    end,
  })
  vim.api.nvim_create_autocmd("BufWinEnter", {
    buffer = buf,
    callback = function()
      vim.wo.conceallevel = 2
      vim.wo.concealcursor = "nvic"
    end,
  })
  vim.api.nvim_create_autocmd("CursorMoved", { buffer = buf, callback = clamp })
  vim.api.nvim_create_autocmd({ "TextChanged", "TextChangedI" }, {
    buffer = buf,
    callback = function()
      decorate(buf)
    end,
  })
  vim.keymap.set("n", "<CR>", function()
    open_task(buf)
  end, { buffer = buf, silent = true })
  vim.keymap.set("n", "g<Space>", toggle, { buffer = buf, silent = true })
  vim.keymap.set("n", "g?", function()
    notify(
      "Edit lines to add, delete, rename, reorder, or indent. :w reviews changes. <CR> opens a task; g<Space> toggles it.",
      vim.log.levels.INFO
    )
  end, { buffer = buf, silent = true })
end

function M.open(project)
  local name = "tt://" .. project.slug
  local buf = vim.fn.bufnr(name)
  if buf == -1 then
    buf = vim.api.nvim_create_buf(true, false)
    vim.api.nvim_buf_set_name(buf, name)
  end
  if not projects[buf] then
    initialize(buf, project)
    if not reload(buf) then
      projects[buf] = nil
      vim.api.nvim_buf_delete(buf, { force = true })
      return
    end
  end
  vim.api.nvim_set_current_buf(buf)
end

function M.project(buf)
  return projects[buf]
end

function M.refresh(path)
  for buf, project in pairs(projects) do
    if vim.api.nvim_buf_is_valid(buf) and project.path == path and not vim.bo[buf].modified then
      reload(buf)
    end
  end
end

return M
