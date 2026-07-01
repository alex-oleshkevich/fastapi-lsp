-- Neovim 0.11+ integration. Drop into ~/.config/nvim/after/plugin/fastapi_lsp.lua
-- or your init.lua. Runs alongside pyright — register both.
vim.lsp.config('fastapi_lsp', {
  cmd = { 'fastapi-lsp', 'lsp' },
  filetypes = { 'python' },
  root_markers = { 'pyproject.toml', 'fastapi-lsp.toml', '.git' },
})
vim.lsp.enable('fastapi_lsp')
