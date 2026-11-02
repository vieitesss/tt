local root = vim.fn.getcwd()
vim.opt.runtimepath:prepend(root)

local tests, failures, skipped = 0, 0, 0
local SKIP = {}
local function test(name, fn)
  tests = tests + 1
  local ok, result = xpcall(fn, debug.traceback)
  if ok and result == SKIP then
    skipped = skipped + 1
    print("SKIP " .. name .. " (filesystem permits writes despite permissions)")
  elseif ok then
    print("PASS " .. name)
  else
    failures = failures + 1
    print("FAIL " .. name .. "\n" .. result)
  end
end

local function eq(expected, actual)
  if not vim.deep_equal(expected, actual) then
    error("expected " .. vim.inspect(expected) .. "\nactual   " .. vim.inspect(actual), 2)
  end
end

test("render and parse preserve tasks with trailing metadata-like titles", function()
  local format = require("tt.format")
  local tasks = {
    {
      id = "a",
      title = "Ship #literal",
      state = "open",
      tags = { "work" },
      priority = "high",
      due = "2026-11-10",
      children = {},
    },
    { id = "b", title = "Done !med", state = "done", tags = {}, children = {} },
    { id = "c", title = "Cancelled @today", state = "cancelled", tags = {}, children = {} },
  }
  local lines, originals = format.render(tasks)
  eq(
    { "/a - [ ] Ship \\#literal #work !high @2026-11-10", "/b - [x] Done \\!med", "/c - [-] Cancelled \\@today" },
    lines
  )
  local parsed, err = format.parse(lines, originals)
  assert(parsed, err)
  eq(tasks, parsed)
end)

test("lossy titles and literal backslashes round-trip without changes", function()
  local format = require("tt.format")
  local tasks = {
    { id = "a", title = "  multi\nline #tag  ", state = "open", tags = {}, children = {} },
    { id = "b", title = "literal \\#tag", state = "open", tags = {}, children = {} },
  }
  local lines, originals = format.render(tasks)
  local parsed, err = format.parse(lines, originals)
  assert(parsed, err)
  eq(tasks, parsed)
  lines[1] = lines[1]:gsub("multi", "changed")
  parsed = assert(format.parse(lines, originals))
  eq("changed line #tag", parsed[1].title)
end)

test("parse accepts plain and checked new tasks, nesting, dates, and duplicate copies", function()
  local format = require("tt.format")
  local parsed, err = format.parse({
    "/a - [ ] Root",
    "      /b - [x] Child #one #two !med @today",
    "      /b - [x] Child #one #two !med @today",
    "  - [-] Cancelled @tomorrow",
    "Plain text",
    "",
  })
  assert(parsed, err)
  eq(2, #parsed)
  eq(3, #parsed[1].children)
  eq("b", parsed[1].children[1].id)
  eq(nil, parsed[1].children[2].id)
  eq({ "one", "two" }, parsed[1].children[1].tags)
  eq(os.date("%Y-%m-%d"), parsed[1].children[1].due)
  local tomorrow = os.date("*t")
  tomorrow.day = tomorrow.day + 1
  eq(os.date("%Y-%m-%d", os.time(tomorrow)), parsed[1].children[3].due)
  eq("cancelled", parsed[1].children[3].state)
  eq("Plain text", parsed[2].title)
end)

test("deleting a parent reparents surviving lines by indentation as documented", function()
  local format = require("tt.format")
  local parsed = assert(format.parse({ "/a - [ ] A", "  /c - [ ] C" }))
  eq(1, #parsed)
  eq("a", parsed[1].id)
  eq("c", parsed[1].children[1].id)

  parsed = assert(format.parse({ "/a - [ ] A", "/c - [ ] C" }))
  eq(2, #parsed)
  eq("c", parsed[2].id)

  parsed = assert(format.parse({ "  /c - [ ] C" }))
  eq("c", parsed[1].id)

  local readme = table.concat(vim.fn.readfile(root .. "/README.md"), "\n")
  assert(readme:find("attach to the preceding less-indented line", 1, true), "README must describe reparenting")
  assert(not readme:find("they move to the root", 1, true), "README must not promise promotion to root")
end)

test("malformed lines report their number", function()
  local format = require("tt.format")
  for _, line in ipairs({ "- [z] broken", "- [ ]", "  \t- [ ] tab", "/bad! - [ ] bad", "- [ ] No date @2026-02-31" }) do
    local nodes, err = format.parse({ "Valid", line })
    eq(nil, nodes)
    assert(err:find("line 2:", 1, true), err)
  end
end)

test("unsupported whitespace in stored tags cannot cause data loss", function()
  local format = require("tt.format")
  local lines, err = format.render({
    { id = "a", title = "Safe", state = "open", tags = { "two words" }, children = {} },
  })
  eq(nil, lines)
  assert(err:find("two words", 1, true))
end)

local sandbox = vim.fn.tempname()
local project = sandbox .. "/project"
vim.fn.mkdir(project, "p")
vim.env.TT_CONFIG = sandbox .. "/config.toml"
vim.env.XDG_DATA_HOME = sandbox .. "/data"
vim.env.XDG_CONFIG_HOME = sandbox .. "/config"
vim.env.TT_PATH = nil
vim.env.TT_VAULT = nil
local bin = root .. "/bin/tt"
local function cli(args)
  local argv = { bin, "--json", "--path", project }
  vim.list_extend(argv, args)
  local result = vim.system(argv, { text = true }):wait()
  assert(result.code == 0, result.stdout .. result.stderr)
  return vim.json.decode(result.stdout)
end
vim.cmd("runtime plugin/tt.lua")

test("write renames a task while preserving its body", function()
  cli({ "project", "add", project })
  local task = cli({ "add", "Original", "--body", "Important body\n" })
  vim.cmd("cd " .. vim.fn.fnameescape(project))
  vim.cmd("TT")
  local line = vim.api.nvim_get_current_line()
  assert(line:find(task.id, 1, true))
  vim.api.nvim_set_current_line((line:gsub("Original", "Renamed")))
  local confirm = vim.fn.confirm
  vim.fn.confirm = function()
    return 1
  end
  vim.cmd("write")
  vim.fn.confirm = confirm
  local shown = cli({ "show", task.id })
  eq("Renamed", shown.title)
  eq("Important body\n", shown.body)
  assert(not vim.bo.modified)
end)

local serial = 0
local function fresh()
  serial = serial + 1
  project = sandbox .. "/project-" .. serial
  vim.fn.mkdir(project, "p")
  vim.cmd("enew!")
  vim.cmd("cd " .. vim.fn.fnameescape(project))
  cli({ "project", "add", project })
end

local function open()
  vim.cmd("TT")
end

local function line_of(id)
  for n, line in ipairs(vim.api.nvim_buf_get_lines(0, 0, -1, false)) do
    if line:match("^%s*/" .. id .. "%s") then
      return n
    end
  end
  error("task not found in buffer: " .. id)
end

local function on(id)
  vim.api.nvim_win_set_cursor(0, { line_of(id), 0 })
end

local function write_with(answer)
  local confirm, prompt = vim.fn.confirm
  vim.fn.confirm = function(message)
    prompt = message
    return answer
  end
  local ok, err = pcall(vim.cmd, "write")
  vim.fn.confirm = confirm
  assert(ok, err)
  return prompt
end

test("dd deletes a task and warns when it has a body", function()
  fresh()
  local task = cli({ "add", "Delete me", "--body", "Important body" })
  open()
  on(task.id)
  vim.cmd("normal! dd")
  local prompt = write_with(1)
  assert(prompt:find("DELETES: 1", 1, true), prompt)
  assert(prompt:find("Delete me [HAS BODY]", 1, true), prompt)
  eq(0, #cli({ "list" }).tasks)
end)

test("dd and p reorder tasks without losing identity or body", function()
  fresh()
  local first = cli({ "add", "First", "--body", "Preserve me\n" })
  local second = cli({ "add", "Second" })
  open()
  on(first.id)
  vim.cmd("normal! dd")
  on(second.id)
  vim.cmd("normal! p")
  assert(write_with(1):find("Reorders:", 1, true))
  local tasks = cli({ "list" }).tasks
  eq(second.id, tasks[1].id)
  eq(first.id, tasks[2].id)
  eq("Preserve me\n", cli({ "show", first.id }).body)
end)

test("indent reparents an existing task", function()
  fresh()
  local parent = cli({ "add", "Parent" })
  local child = cli({ "add", "Child", "--body", "Child body" })
  open()
  on(child.id)
  vim.cmd("normal! >>")
  write_with(1)
  local tree = cli({ "list" }).tasks
  eq(parent.id, tree[1].id)
  eq(child.id, tree[1].children[1].id)
  eq("Child body", cli({ "show", child.id }).body)
end)

test("yyp creates a copy and keeps the original task", function()
  fresh()
  local task = cli({ "add", "Copy me", "--body", "Original body" })
  open()
  on(task.id)
  vim.cmd("normal! yyp")
  write_with(1)
  local tasks = cli({ "list" }).tasks
  eq(2, #tasks)
  eq(task.id, tasks[1].id)
  assert(tasks[2].id ~= task.id)
  eq("Copy me", tasks[2].title)
  eq("Original body", cli({ "show", task.id }).body)
  eq("", cli({ "show", tasks[2].id }).body)
end)

test("new checked and plain lines create tasks with metadata", function()
  fresh()
  open()
  vim.api.nvim_buf_set_lines(0, 0, -1, false, { "- [x] Checked #tag !low @2026-11-10", "Plain task" })
  write_with(1)
  local tasks = cli({ "list" }).tasks
  eq(2, #tasks)
  local checked, plain = tasks[1], tasks[2]
  eq("done", checked.state)
  eq({ "tag" }, checked.tags)
  eq("low", checked.priority)
  eq("2026-11-10", checked.due)
  eq("Plain task", plain.title)
  eq("open", plain.state)
end)

test("cancel keeps modified text and never changes the store", function()
  fresh()
  local task = cli({ "add", "Before" })
  open()
  vim.api.nvim_set_current_line((vim.api.nvim_get_current_line():gsub("Before", "After")))
  local line = vim.api.nvim_get_current_line()
  assert(write_with(2))
  eq("Before", cli({ "show", task.id }).title)
  eq(line, vim.api.nvim_get_current_line())
  assert(vim.bo.modified)
end)

test("no-change write does not prompt and clears modified", function()
  fresh()
  cli({ "add", "Unchanged" })
  open()
  local line = vim.api.nvim_get_current_line()
  vim.api.nvim_set_current_line(line .. " ")
  assert(vim.bo.modified)
  eq(nil, write_with(1))
  assert(not vim.bo.modified)
end)

test("stale revision reports reload and preserves text", function()
  fresh()
  local task = cli({ "add", "Before" })
  open()
  vim.api.nvim_set_current_line((vim.api.nvim_get_current_line():gsub("Before", "Local edit")))
  local line = vim.api.nvim_get_current_line()
  cli({ "edit", task.id, "--title=External edit" })
  local messages, notify = {}, vim.notify
  vim.notify = function(message)
    messages[#messages + 1] = message
  end
  eq(nil, write_with(1))
  vim.notify = notify
  assert(table.concat(messages):find("task list changed on disk; reload", 1, true))
  eq(line, vim.api.nvim_get_current_line())
  assert(vim.bo.modified)
  eq("External edit", cli({ "show", task.id }).title)
end)

test("enter opens the task file and unsaved lines are not opened", function()
  fresh()
  local task = cli({ "add", "Open me" })
  open()
  vim.api.nvim_buf_set_lines(0, -1, -1, false, { "Unsaved" })
  vim.api.nvim_win_set_cursor(0, { 2, 0 })
  local messages, notify = {}, vim.notify
  vim.notify = function(message)
    messages[#messages + 1] = message
  end
  vim.api.nvim_feedkeys(vim.api.nvim_replace_termcodes("<CR>", true, false, true), "xt", false)
  vim.notify = notify
  assert(table.concat(messages):find("save this task", 1, true))
  on(task.id)
  vim.api.nvim_feedkeys(vim.api.nvim_replace_termcodes("<CR>", true, false, true), "xt", false)
  eq(vim.uv.fs_realpath(task.path), vim.uv.fs_realpath(vim.api.nvim_buf_get_name(0)))
end)

test("identity prefix is concealed and cursor is clamped past it", function()
  fresh()
  local task = cli({ "add", "Conceal" })
  open()
  local line = vim.api.nvim_get_current_line()
  eq(2, vim.wo.conceallevel)
  eq("nvic", vim.wo.concealcursor)
  eq(1, vim.fn.synconcealed(1, 1)[1])
  vim.api.nvim_win_set_cursor(0, { 1, 0 })
  vim.api.nvim_exec_autocmds("CursorMoved", { buffer = 0 })
  eq(#("/" .. task.id .. " "), vim.api.nvim_win_get_cursor(0)[2])
  assert(line:find("/" .. task.id .. " ", 1, true) == 1)
end)

test("linked worktree and main checkout open the same task buffer", function()
  fresh()
  local main = project
  local linked = sandbox .. "/linked-worktree"
  local function git(args)
    local argv = { "git", "-C", main }
    vim.list_extend(argv, args)
    local result = vim.system(argv, { text = true }):wait()
    assert(result.code == 0, result.stderr)
  end
  git({ "init", "-q" })
  git({ "-c", "user.name=Test", "-c", "user.email=test@example.com", "commit", "--allow-empty", "-qm", "init" })
  git({ "worktree", "add", "-qb", "test-worktree", linked })
  local task = cli({ "add", "Shared" })
  open()
  local buf = vim.api.nvim_get_current_buf()
  local name = vim.api.nvim_buf_get_name(buf)
  vim.cmd("enew!")
  vim.cmd("cd " .. vim.fn.fnameescape(linked))
  open()
  eq(buf, vim.api.nvim_get_current_buf())
  eq(name, vim.api.nvim_buf_get_name(0))
  assert(vim.api.nvim_get_current_line():find(task.id, 1, true))
end)

test("reading twice leaves task files byte-identical", function()
  fresh()
  local task = cli({ "add", "Read only", "--body", "Body\nwith lines\n" })
  local function bytes()
    local file = assert(io.open(task.path, "rb"))
    local result = file:read("*a")
    file:close()
    return result
  end
  local original = bytes()
  open()
  eq(original, bytes())
  vim.cmd("edit!")
  eq(original, bytes())
  local buf = vim.api.nvim_get_current_buf()
  vim.cmd("enew!")
  vim.api.nvim_set_current_buf(buf)
  eq(original, bytes())
end)

test("unregistered project prompts, cancels, then registers from the current file directory", function()
  local directory = sandbox .. "/unregistered"
  vim.fn.mkdir(directory, "p")
  vim.cmd("enew!")
  vim.cmd("cd " .. vim.fn.fnameescape(sandbox))
  vim.cmd.edit(vim.fn.fnameescape(directory .. "/file.txt"))
  local confirm = vim.fn.confirm
  vim.fn.confirm = function()
    return 2
  end
  vim.cmd("TT")
  eq("file.txt", vim.fn.expand("%:t"))
  project = directory
  assert(not cli({ "project", "current" }).registered)
  vim.fn.confirm = function()
    return 1
  end
  vim.cmd("TT")
  vim.fn.confirm = confirm
  assert(vim.api.nvim_buf_get_name(0):match("^tt://"))
  assert(cli({ "project", "current" }).registered)
end)

test("TT add reloads a clean buffer but preserves a modified one", function()
  fresh()
  open()
  vim.cmd("TT add First from command")
  local tasks = cli({ "list" }).tasks
  eq("First from command", tasks[1].title)
  assert(vim.api.nvim_get_current_line():find(tasks[1].id, 1, true))
  vim.api.nvim_set_current_line((vim.api.nvim_get_current_line():gsub("First", "Local")))
  local line = vim.api.nvim_get_current_line()
  vim.cmd("TT add Second from command")
  eq(line, vim.api.nvim_get_current_line())
  assert(vim.bo.modified)
  eq(2, #cli({ "list" }).tasks)
end)

test("TT projects opens the selected project", function()
  fresh()
  local task = cli({ "add", "Selected task" })
  local target = cli({ "project", "current" }).slug
  vim.cmd("enew!")
  local select_ui = vim.ui.select
  vim.ui.select = function(items, _, callback)
    for _, item in ipairs(items) do
      if item.slug == target then
        return callback(item)
      end
    end
    error("project missing from selection")
  end
  vim.cmd("TT projects")
  vim.ui.select = select_ui
  assert(vim.api.nvim_buf_get_name(0):find("tt://" .. target, 1, true))
  assert(vim.api.nvim_get_current_line():find(task.id, 1, true))
end)

test("toggle key changes the checkbox and write applies the state", function()
  fresh()
  local task = cli({ "add", "Toggle me" })
  open()
  vim.api.nvim_feedkeys(vim.api.nvim_replace_termcodes("g<Space>", true, false, true), "xt", false)
  assert(vim.api.nvim_get_current_line():find("[x]", 1, true))
  write_with(1)
  eq("done", cli({ "show", task.id }).state)
end)

test("partial apply failure reports progress and keeps the buffer", function()
  fresh()
  local task = cli({ "add", "Before failure" })
  open()
  vim.api.nvim_set_current_line((vim.api.nvim_get_current_line():gsub("Before", "After")))
  local line = vim.api.nvim_get_current_line()
  local directory = vim.fn.fnamemodify(task.path, ":h")
  assert(vim.uv.fs_chmod(directory, tonumber("555", 8)))
  local probe = io.open(directory .. "/permission-probe", "wb")
  assert(vim.uv.fs_chmod(directory, tonumber("755", 8)))
  if probe then
    probe:close()
    os.remove(directory .. "/permission-probe")
    return SKIP
  end
  local confirm, notify, messages = vim.fn.confirm, vim.notify, {}
  vim.fn.confirm = function()
    assert(vim.uv.fs_chmod(directory, tonumber("555", 8)))
    return 1
  end
  vim.notify = function(message)
    messages[#messages + 1] = message
  end
  local ok, err = pcall(vim.cmd, "write")
  assert(vim.uv.fs_chmod(directory, tonumber("755", 8)))
  vim.fn.confirm, vim.notify = confirm, notify
  assert(ok, err)
  assert(table.concat(messages):find("Already applied:", 1, true), vim.inspect(messages))
  eq(line, vim.api.nvim_get_current_line())
  assert(vim.bo.modified)
  eq("Before failure", cli({ "show", task.id }).title)
end)

test("BufEnter refreshes clean buffers but keeps modified text", function()
  fresh()
  local task = cli({ "add", "Initial" })
  open()
  local buf = vim.api.nvim_get_current_buf()
  vim.cmd("enew!")
  cli({ "edit", task.id, "--title=External" })
  vim.api.nvim_set_current_buf(buf)
  assert(vim.api.nvim_get_current_line():find("External", 1, true))
  vim.api.nvim_set_current_line((vim.api.nvim_get_current_line():gsub("External", "Local")))
  local line = vim.api.nvim_get_current_line()
  vim.cmd("enew!")
  cli({ "edit", task.id, "--title=External again" })
  vim.api.nvim_set_current_buf(buf)
  eq(line, vim.api.nvim_get_current_line())
  assert(vim.bo.modified)
end)

test("malformed write reports the line and applies nothing", function()
  fresh()
  local task = cli({ "add", "Safe" })
  open()
  vim.api.nvim_buf_set_lines(0, -1, -1, false, { "- [z] Broken" })
  local messages, notify = {}, vim.notify
  vim.notify = function(message)
    messages[#messages + 1] = message
  end
  eq(nil, write_with(1))
  vim.notify = notify
  assert(table.concat(messages):find("line 2:", 1, true))
  eq("Safe", cli({ "show", task.id }).title)
  assert(vim.bo.modified)
end)

test("configured missing binary reports the build command", function()
  fresh()
  local notify, messages = vim.notify, {}
  vim.notify = function(message)
    messages[#messages + 1] = message
  end
  require("tt").setup({ bin = sandbox .. "/missing-tt" })
  vim.cmd("TT")
  require("tt").setup({})
  vim.notify = notify
  assert(table.concat(messages):find("go build -o bin/tt ./cmd/tt", 1, true))
  assert(not vim.api.nvim_buf_get_name(0):match("^tt://"))
end)

test("unrepresentable stored tag prevents opening a writable list", function()
  fresh()
  local task = cli({ "add", "Protected", "--tag", "two words" })
  local notify, messages = vim.notify, {}
  vim.notify = function(message)
    messages[#messages + 1] = message
  end
  vim.cmd("TT")
  vim.cmd("TT")
  vim.notify = notify
  assert(table.concat(messages):find("tag containing whitespace", 1, true))
  assert(not vim.api.nvim_buf_get_name(0):match("^tt://"))
  eq({ "two words" }, cli({ "show", task.id }).tags)
end)

test("project and task paths with spaces open correctly", function()
  project = sandbox .. "/project with spaces"
  vim.fn.mkdir(project, "p")
  vim.cmd("enew!")
  vim.cmd("cd " .. vim.fn.fnameescape(project))
  cli({ "project", "add", project })
  local task = cli({ "add", "Open spaced path" })
  open()
  local buf = vim.api.nvim_get_current_buf()
  vim.cmd("enew!")
  open()
  eq(buf, vim.api.nvim_get_current_buf())
  on(task.id)
  vim.api.nvim_feedkeys(vim.api.nvim_replace_termcodes("<CR>", true, false, true), "xt", false)
  eq(vim.uv.fs_realpath(task.path), vim.uv.fs_realpath(vim.api.nvim_buf_get_name(0)))
end)

test("project names containing pattern characters reuse their exact buffer", function()
  project = sandbox .. "/project [one]"
  vim.fn.mkdir(project, "p")
  vim.cmd("enew!")
  vim.cmd("cd " .. vim.fn.fnameescape(project))
  cli({ "project", "add", project })
  open()
  local buf = vim.api.nvim_get_current_buf()
  vim.cmd("enew!")
  open()
  eq(buf, vim.api.nvim_get_current_buf())
end)

test("renaming preserves unknown frontmatter fields", function()
  fresh()
  local task = cli({ "add", "Extra field", "--body", "Body remains\n" })
  local file = assert(io.open(task.path, "rb"))
  local original = file:read("*a")
  file:close()
  local changed = original:gsub("\n%-%-%-\n", "\ncustom: retained\n---\n", 1)
  file = assert(io.open(task.path, "wb"))
  file:write(changed)
  file:close()
  open()
  vim.api.nvim_set_current_line((vim.api.nvim_get_current_line():gsub("Extra field", "Renamed")))
  write_with(1)
  file = assert(io.open(task.path, "rb"))
  local after = file:read("*a")
  file:close()
  assert(after:find("custom: retained", 1, true))
  eq("Body remains\n", cli({ "show", task.id }).body)
end)

test("lossy displayed title survives an unrelated write", function()
  fresh()
  local title = "  multi\nline #literal  "
  local task = cli({ "add", "--title=" .. title })
  local other = cli({ "add", "Rename me" })
  open()
  on(other.id)
  vim.api.nvim_set_current_line((vim.api.nvim_get_current_line():gsub("Rename me", "Renamed")))
  write_with(1)
  eq(title, cli({ "show", task.id }).title)
end)

test("project pattern characters cannot select a different project buffer", function()
  project = sandbox .. "/sameX"
  vim.fn.mkdir(project, "p")
  vim.cmd("enew!")
  vim.cmd("cd " .. vim.fn.fnameescape(project))
  cli({ "project", "add", project })
  cli({ "add", "First project" })
  open()
  local first = vim.api.nvim_get_current_buf()
  project = sandbox .. "/same."
  vim.fn.mkdir(project, "p")
  vim.cmd("enew!")
  vim.cmd("cd " .. vim.fn.fnameescape(project))
  cli({ "project", "add", project })
  cli({ "add", "Second project" })
  open()
  assert(first ~= vim.api.nvim_get_current_buf())
  assert(vim.api.nvim_get_current_line():find("Second project", 1, true))
end)

test("toggle restores a missing checkbox without losing task identity", function()
  fresh()
  local task = cli({ "add", "Existing", "--body", "keep me" })
  open()
  vim.api.nvim_set_current_line((vim.api.nvim_get_current_line():gsub("%- %[%s%] ", "")))
  vim.api.nvim_feedkeys(vim.api.nvim_replace_termcodes("g<Space>", true, false, true), "xt", false)
  write_with(1)
  local tasks = cli({ "list" }).tasks
  eq(1, #tasks)
  eq(task.id, tasks[1].id)
  eq("done", tasks[1].state)
  eq("keep me", cli({ "show", task.id }).body)
end)

vim.fn.delete(sandbox, "rf")
print(string.format("%d tests, %d failures, %d skipped", tests, failures, skipped))
if failures > 0 then
  vim.cmd("cquit 1")
end
vim.cmd("qa!")
