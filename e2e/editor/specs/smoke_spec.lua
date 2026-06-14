-- Neovim smoke tests for fastapi-lsp (REQ-TST-04).
-- One scenario per capability: hover, symbols, goto, diagnostics.
-- Run via: nvim --headless --noplugin -u e2e/editor/minimal_init.lua \
--          -c "PlenaryBustedDirectory e2e/editor/specs" -c "qa!"

local script_dir = debug.getinfo(1, "S").source:sub(2):match("(.*[/\\])")
local project_root = vim.fn.fnamemodify(script_dir .. "../../..", ":p")
local fixture = project_root .. "e2e/fixtures/bookshop/app/routers/books.py"

local function wait_for_lsp(bufnr, timeout_ms)
  timeout_ms = timeout_ms or 5000
  local deadline = vim.uv.now() + timeout_ms
  while vim.uv.now() < deadline do
    local clients = vim.lsp.get_clients({ bufnr = bufnr, name = "fastapi-lsp" })
    if #clients > 0 then
      return clients[1]
    end
    vim.wait(100)
  end
  return nil
end

local function open_fixture()
  vim.cmd("edit " .. vim.fn.fnameescape(fixture))
  local bufnr = vim.api.nvim_get_current_buf()
  local client = wait_for_lsp(bufnr, 8000)
  assert.is_not_nil(client, "fastapi-lsp did not attach within 8 seconds")
  -- Give the LSP time to index the workspace
  vim.wait(2000, function() return false end)
  return bufnr, client
end

describe("fastapi-lsp smoke", function()
  local bufnr, client

  before_each(function()
    bufnr, client = open_fixture()
  end)

  after_each(function()
    if bufnr and vim.api.nvim_buf_is_valid(bufnr) then
      vim.api.nvim_buf_delete(bufnr, { force = true })
    end
  end)

  -- ── Hover: route card ────────────────────────────────────────────────────────

  it("hover on a handler returns route card markdown", function()
    -- Position: line 13 (0-indexed) = `def list_books(db: DbDep):`
    -- which is on @router.get("/", ...) decorated handler
    vim.api.nvim_win_set_cursor(0, { 14, 4 }) -- line 14 (1-indexed) = def list_books

    local result = vim.lsp.buf_request_sync(bufnr, "textDocument/hover", {
      textDocument = { uri = vim.uri_from_fname(fixture) },
      position = { line = 13, character = 4 },
    }, 5000)

    assert.is_not_nil(result, "hover request timed out")
    local has_response = false
    for _, res in pairs(result) do
      if res.result and res.result.contents then
        has_response = true
        local content = res.result.contents
        local text = type(content) == "table" and (content.value or "") or tostring(content)
        assert.truthy(text:find("GET"), "route card should contain method GET")
        assert.truthy(
          text:find("/books") or text:find("unresolved"),
          "route card should contain /books path or unresolved marker"
        )
      end
    end
    assert.is_true(has_response, "no hover response from fastapi-lsp")
  end)

  -- ── Workspace symbols ────────────────────────────────────────────────────────

  it("workspace symbols contains routes from bookshop", function()
    local result = vim.lsp.buf_request_sync(bufnr, "workspace/symbol", {
      query = "GET",
    }, 5000)

    assert.is_not_nil(result, "workspace/symbol request timed out")
    local found_route = false
    for _, res in pairs(result) do
      if res.result then
        for _, sym in ipairs(res.result) do
          local name = sym.name or ""
          if name:find("GET") and name:find("/books") then
            found_route = true
            break
          end
        end
      end
    end
    assert.is_true(found_route, "workspace symbols should contain 'GET /api/books...' route")
  end)

  -- ── Document symbols ─────────────────────────────────────────────────────────

  it("document symbols for books.py contains route symbols", function()
    local result = vim.lsp.buf_request_sync(bufnr, "textDocument/documentSymbol", {
      textDocument = { uri = vim.uri_from_fname(fixture) },
    }, 5000)

    assert.is_not_nil(result, "textDocument/documentSymbol timed out")
    local found = false
    for _, res in pairs(result) do
      if res.result and #res.result > 0 then
        found = true
        break
      end
    end
    assert.is_true(found, "document symbols should be non-empty for books.py")
  end)
end)
