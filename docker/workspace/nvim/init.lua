-- Minimal LazyVim setup, shared by every workspace. Language extras are
-- enabled when the workspace has that toolchain, so the Go workspace gets gopls
-- and the Python workspace gets pyright without separate configs.
local lazypath = vim.fn.stdpath("data") .. "/lazy/lazy.nvim"
if not (vim.uv or vim.loop).fs_stat(lazypath) then
  vim.fn.system({ "git", "clone", "--filter=blob:none", "--branch=stable", "https://github.com/folke/lazy.nvim.git", lazypath })
end
vim.opt.rtp:prepend(lazypath)

vim.g.mapleader = " "
vim.g.maplocalleader = "\\"

local spec = {
  { "LazyVim/LazyVim", import = "lazyvim.plugins" },
  { import = "lazyvim.plugins.extras.lang.json" },
  { import = "lazyvim.plugins.extras.lang.yaml" },
  { import = "lazyvim.plugins.extras.lang.docker" },
}
local toolchains = {
  go = "lazyvim.plugins.extras.lang.go",
  python3 = "lazyvim.plugins.extras.lang.python",
  cargo = "lazyvim.plugins.extras.lang.rust",
}
for exe, extra in pairs(toolchains) do
  if vim.fn.executable(exe) == 1 then
    table.insert(spec, { import = extra })
  end
end

require("lazy").setup({
  spec = spec,
  defaults = { lazy = false, version = false },
  install = { colorscheme = { "tokyonight", "habamax" } },
  checker = { enabled = false },
})
