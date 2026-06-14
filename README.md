# fastapi-lsp

A language server that understands FastAPI and Starlette the way a framework expert does — route resolution through router prefixes, the `Depends()` graph, `url_for` reverse routing, Jinja template links, test-to-route navigation, and env/settings intelligence. One Rust binary, any LSP-capable editor.

> **Status: specification phase.** The design lives in [`specs/`](specs/index.md); implementation tracks the [roadmap](specs/roadmap.md).

## What it does

A type checker sees `@app.get("/books/{book_id}")` as a decorator taking a string. This server sees a route. It complements Pyright/ty — it never duplicates type checking, and only adds what framework indexing can know:

- **Routes as symbols** — search `GET /api/books/{book_id}` in your editor's symbol picker; full paths resolved through every `include_router` prefix
- **Diagnostics** — path-param mismatches, duplicate/shadowed routes, never-included routers, `Depends(fn())` footguns, dependency cycles, broken `url_for` names (in Python *and* templates), missing templates, undefined env keys
- **Navigation** — ctrl-click from `client.get("/api/books/1")` to the handler, through `Depends()` in both directions, into template files, onto `.env` lines
- **Hover** — the full route card: resolved path, router chain, response model, dependencies, applied middleware
- **Completions** — route paths in test calls, route names in `url_for`, template paths, env keys, middleware kwargs
- **Code actions** — quick fixes for the diagnostics, extract-router, extract named dependency, create model, test stubs
- **`check` mode** — the same diagnostics as a CLI linter for CI: `fastapi-lsp check . --ignore env/undefined-key`

Static analysis only: the server never imports or executes your code.

## Editor setup

The binary must be on `PATH` (`cargo install fastapi-lsp`, `pip install fastapi-lsp`, or download a release binary).

### Neovim

```lua
-- nvim-lspconfig
vim.lsp.config('fastapi_lsp', {
  cmd = { 'fastapi-lsp', '--stdio' },
  filetypes = { 'python', 'html', 'htmldjango' },
  root_markers = { 'pyproject.toml', '.git' },
})
vim.lsp.enable('fastapi_lsp')
```

The `html` / `htmldjango` filetypes are load-bearing: without them the server is never attached to `.html` buffers, so template diagnostics, completions, and navigation never fire inside templates.

### Helix

```toml
# ~/.config/helix/languages.toml
[language-server.fastapi-lsp]
command = "fastapi-lsp"
args = ["--stdio"]

[[language]]
name = "python"
language-servers = ["pyright", "fastapi-lsp"]

[[language]]
name = "html"
language-servers = ["vscode-html-language-server", "fastapi-lsp"]

[[language]]
name = "jinja"
language-servers = ["fastapi-lsp"]
```

Order matters in Helix. It routes hover, goto-definition, and references to the *first* server that advertises the capability; diagnostics, completion, code actions, and symbols merge across servers. With the type checker first (as above), its hover and goto stay primary — framework hover cards and string-goto are unavailable, while diagnostics, completion, actions, and symbols still work. List `fastapi-lsp` first to invert this trade-off.

### Zed

Install the extension from `editors/zed/` for local dev:

```bash
./scripts/install-zed-extension.sh
```

Then opt in — Zed runs the extension alongside the default Python server only when explicitly named:

```jsonc
// ~/.config/zed/settings.json
{
  "languages": {
    "Python": { "language_servers": ["fastapi-lsp", "..."] }
  }
}
```

Pass initialization options through the `lsp` key:

```jsonc
{
  "lsp": {
    "fastapi-lsp": {
      "initialization_options": { "templates": ["app/templates"] }
    }
  }
}
```

### Troubleshooting

**Server never starts** — verify `fastapi-lsp --stdio` is on `PATH`. Run it manually: a blank `Content-Length: …` handshake on stdin proves the binary is working.

**Template features missing** — check that the `html`/`htmldjango`/`jinja` filetypes are listed in your editor config (see snippets above). The server must be attached to the template buffer to answer template requests.

**Diagnostics look wrong** — the server emits `source: "fastapi-lsp"` on every diagnostic; filter by that to isolate its output from the type checker's.

## Configuration

Zero config works for plain projects. Otherwise, one schema from three sources (editor `InitializationOptions` › `fastapi-lsp.toml` › `[tool.fastapi-lsp]` in `pyproject.toml`):

```toml
# fastapi-lsp.toml
entrypoint = "app/main.py"
templates = ["templates"]
env_files = [".env", ".env.example"]

[features]
code_lens = false          # any capability can be switched off
```

Full schema: [specs/foundations/E15-app-config.md](specs/foundations/E15-app-config.md).

## CLI

```
fastapi-lsp lsp [--stdio | --http --address 127.0.0.1 --port 9257]
fastapi-lsp check PATH [--only CODES] [--ignore CODES] [--format text|json]
```

`check` exits non-zero when Warning-or-worse findings exist — same engine, same findings as the editor.

## Development

```bash
cargo build                                  # build
cargo test                                   # unit tests: extractors + linking
cargo build && uv run pytest e2e/ -v         # e2e: pytest-lsp protocol suite
RUST_LOG=debug ./target/debug/fastapi-lsp lsp --stdio   # manual run
```

The full design — architecture, data model, every feature — is in [`specs/`](specs/index.md). Start at the index; `specs/foundations/E01-architecture.md` explains the two-pass indexing that everything else builds on.

## License

MIT
