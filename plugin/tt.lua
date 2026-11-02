vim.api.nvim_create_user_command("TT", function(opts)
  require("tt").command(opts.args)
end, {
  nargs = "*",
  complete = function()
    return { "add", "projects" }
  end,
})
