local M = {}

local markers = { open = " ", done = "x", cancelled = "-" }
local states = { [" "] = "open", x = "done", ["-"] = "cancelled" }

local function metadata(word)
  if word:match("^#[^%s]+$") then
    return "tag"
  end
  if word == "!high" or word == "!med" or word == "!low" then
    return "priority"
  end
  if word == "@today" or word == "@tomorrow" or word:match("^@%d%d%d%d%-%d%d%-%d%d$") then
    return "due"
  end
end

local function escaped_metadata(word)
  local slashes, rest = word:match("^(\\+)(.+)$")
  return slashes and metadata(rest)
end

local function display_title(title)
  local display = title:gsub("[\r\n]+", " "):match("^%s*(.-)%s*$")
  local last = display:match("(%S+)$")
  if last and (metadata(last) or escaped_metadata(last)) then
    display = display:sub(1, #display - #last) .. "\\" .. last
  end
  return display
end

function M.render(tasks)
  local lines, originals = {}, {}
  local function visit(nodes, depth)
    for _, task in ipairs(nodes) do
      local title = display_title(task.title)
      originals[task.id] = { display = task.title:gsub("[\r\n]+", " "):match("^%s*(.-)%s*$"), original = task.title }
      local line = string.rep("  ", depth) .. "/" .. task.id .. " - [" .. markers[task.state] .. "] " .. title
      for _, tag in ipairs(task.tags or {}) do
        if tag:find("%s") then
          return "task " .. task.id .. " has a tag containing whitespace: " .. tag
        end
        line = line .. " #" .. tag
      end
      if task.priority and task.priority ~= vim.NIL then
        line = line .. " !" .. task.priority
      end
      if task.due and task.due ~= vim.NIL then
        line = line .. " @" .. task.due
      end
      lines[#lines + 1] = line
      local err = visit(task.children or {}, depth + 1)
      if err then
        return err
      end
    end
  end
  local err = visit(tasks, 0)
  if err then
    return nil, err
  end
  return lines, originals
end

local function date_value(word)
  if word == "@today" or word == "@tomorrow" then
    local date = os.date("*t")
    if word == "@tomorrow" then
      date.day = date.day + 1
    end
    return os.date("%Y-%m-%d", os.time(date))
  end
  local year, month, day = word:match("^@(%d%d%d%d)%-(%d%d)%-(%d%d)$")
  local timestamp = os.time({ year = tonumber(year), month = tonumber(month), day = tonumber(day), hour = 12 })
  local value = word:sub(2)
  if not timestamp or os.date("%Y-%m-%d", timestamp) ~= value then
    return nil
  end
  return value
end

function M.parse(lines, originals)
  local roots, stack, seen = {}, {}, {}
  originals = originals or {}
  for number, line in ipairs(lines) do
    if line:match("%S") then
      local indent, text = line:match("^(%s*)(.-)%s*$")
      if indent:find("\t") then
        return nil, "line " .. number .. ": tabs are not supported in indentation"
      end
      local id, rest = text:match("^/([a-z0-9_-]+)%s+(.+)$")
      if id then
        text = rest
        if #id > 64 then
          return nil, "line " .. number .. ": invalid task id"
        end
      elseif text:match("^/%S+%s+") then
        return nil, "line " .. number .. ": invalid task id"
      end
      local marker, title = text:match("^%- %[(.)%]%s+(.+)$")
      if marker then
        if not states[marker] then
          return nil, "line " .. number .. ": invalid checkbox"
        end
        text = title
      elseif text:match("^%- %[") then
        return nil, "line " .. number .. ": malformed checkbox or missing title"
      end
      local tags, priority, due = {}, nil, nil
      while true do
        local before, word = text:match("^(.*%s)(%S+)$")
        if not word then
          before, word = "", text
        end
        if word == "" then
          break
        end
        if escaped_metadata(word) then
          text = before .. word:sub(2)
          break
        end
        local kind = metadata(word)
        if not kind then
          break
        end
        if kind == "tag" then
          table.insert(tags, 1, word:sub(2))
        elseif kind == "priority" then
          if priority then
            return nil, "line " .. number .. ": multiple priorities"
          end
          priority = word:sub(2)
        else
          if due then
            return nil, "line " .. number .. ": multiple due dates"
          end
          due = date_value(word)
          if not due then
            return nil, "line " .. number .. ": invalid due date"
          end
        end
        text = before:match("^(.-)%s*$")
      end
      text = text:match("^%s*(.-)%s*$")
      if text == "" then
        return nil, "line " .. number .. ": task title must not be empty"
      end
      local original = id and originals[id]
      if original and text == original.display then
        text = original.original
      end
      local node = { title = text, state = states[marker or " "], tags = tags, children = {} }
      if priority then
        node.priority = priority
      end
      if due then
        node.due = due
      end
      if id and not seen[id] then
        node.id = id
      end
      if id then
        seen[id] = true
      end
      local width = #indent
      while #stack > 0 and stack[#stack].indent >= width do
        table.remove(stack)
      end
      local siblings = #stack > 0 and stack[#stack].node.children or roots
      siblings[#siblings + 1] = node
      stack[#stack + 1] = { indent = width, node = node }
    end
  end
  return roots
end

return M
