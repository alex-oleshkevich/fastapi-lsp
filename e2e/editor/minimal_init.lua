-- Minimal Neovim init for fastapi-lsp smoke tests (REQ-TST-04).
-- Usage: nvim --headless --noplugin -u e2e/editor/minimal_init.lua \
--            -c "PlenaryBustedDirectory e2e/editor/specs {minimal_init = 'e2e/editor/minimal_init.lua'}" \
--            -c "qa!"

-- Bootstrap plenary from the user's lazy.nvim install
local plenary_path = vim.fn.expand("~/.local/share/nvim/lazy/plenary.nvim")
vim.opt.rtp:append(plenary_path)
-- Load the plenary plugin commands (not loaded under --noplugin)
vim.cmd("source " .. plenary_path .. "/plugin/plenary.vim")

-- Locate the project root (the directory this file lives in's parent's parent)
local script_dir = debug.getinfo(1, "S").source:sub(2):match("(.*[/\\])")
local project_root = vim.fn.fnamemodify(script_dir .. "../..", ":p")

-- Path to the fastapi-lsp binary (built with `cargo build`)
local lsp_bin = project_root .. "target/debug/fastapi-lsp"

-- Register the LSP for Python files
vim.api.nvim_create_autocmd("FileType", {
  pattern = "python",
  callback = function(args)
    vim.lsp.start({
      name = "fastapi-lsp",
      cmd = { lsp_bin },
      root_dir = project_root .. "e2e/fixtures/bookshop",
      settings = {},
    })
  end,
})

-- Suppress startup messages
vim.opt.shortmess:append("I")
vim.opt.termguicolors = false
