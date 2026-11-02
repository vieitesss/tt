local M = {}
local configured
local root = debug.getinfo(1, "S").source:sub(2):match("^(.*)/lua/tt/cli%.lua$")

function M.setup(bin)
  configured = bin
end

local function binary()
  if configured then
    return configured
  end
  local bundled = root and root .. "/bin/tt"
  if bundled and vim.fn.executable(bundled) == 1 then
    return bundled
  end
  return "tt"
end

function M.run(path, args, input)
  local bin = binary()
  if vim.fn.executable(bin) ~= 1 then
    return nil, { error = "tt binary not found: " .. bin .. "; build it with go build -o bin/tt ./cmd/tt" }
  end
  local argv = { bin, "--json" }
  if path then
    vim.list_extend(argv, { "--path", path })
  end
  vim.list_extend(argv, args)
  local ok, process = pcall(vim.system, argv, { text = true, stdin = input })
  if not ok then
    return nil, { error = tostring(process) }
  end
  local result = process:wait()
  local decoded, response = pcall(vim.json.decode, result.stdout or "")
  if result.code ~= 0 then
    if decoded and type(response) == "table" and response.error then
      return nil, response
    end
    return nil, { error = vim.trim((result.stderr or "") ~= "" and result.stderr or result.stdout or "tt failed") }
  end
  if not decoded or type(response) ~= "table" then
    return nil, { error = "tt returned invalid JSON: " .. (result.stdout or "") }
  end
  return response
end

return M
