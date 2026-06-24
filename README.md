# fastapi-lsp

[![CI](https://github.com/alex-oleshkevich/fastapi-lsp/actions/workflows/ci.yml/badge.svg)](https://github.com/alex-oleshkevich/fastapi-lsp/actions/workflows/ci.yml)
[![Release](https://github.com/alex-oleshkevich/fastapi-lsp/actions/workflows/release.yml/badge.svg)](https://github.com/alex-oleshkevich/fastapi-lsp/releases)

Language server for FastAPI and Starlette — routes, `Depends()` graph, `url_for`, Jinja templates, env/settings. One Rust binary, any LSP-capable editor. Static analysis only.

## Features

| | |
|---|---|
| **Diagnostics** | param mismatches, shadowed routes, unincluded routers, `Depends(fn())`, dep cycles, broken `url_for`, missing templates, undefined env keys |
| **Navigation** | test call → handler, `Depends()` chains both ways, template files, `.env` lines |
| **Hover** | route card: resolved path, router chain, response model, deps, middleware |
| **Completions** | route paths, `url_for` names, template paths, env keys |
| **Symbols** | `GET /api/books/{book_id}` in the symbol picker |
| **Code lenses** | test count, dep usage, override count per handler |
| **`check` CLI** | same diagnostics as a linter — `fastapi-lsp check .` |

## Installation

```bash
uv tool install fastapi-lsp
```

Or with pip:

```bash
pip install fastapi-lsp
```

Or download a pre-built binary from the [releases page](https://github.com/alex-oleshkevich/fastapi-lsp/releases).

## Editor setup

### Neovim

```lua
vim.lsp.config('fastapi_lsp', {
  cmd = { 'fastapi-lsp', '--stdio' },
  filetypes = { 'python', 'html', 'htmldjango' },
  root_markers = { 'pyproject.toml', '.git' },
})
vim.lsp.enable('fastapi_lsp')
```

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

### Zed

Install from the Zed extensions panel (`Cmd+Shift+X`) — search for **fastapi-lsp** and click Install.

Then add to `~/.config/zed/settings.json`:

```jsonc
{
  "languages": { "Python": { "language_servers": ["fastapi-lsp", "..."] } },
  "lsp": { "fastapi-lsp": { "initialization_options": { "templates": ["app/templates"] } } }
}
```

## Configuration

Zero config for standard projects. Priority: `InitializationOptions` › `fastapi-lsp.toml` › `[tool.fastapi-lsp]` in `pyproject.toml`.

| Option | Default | |
|---|---|---|
| `entrypoint` | _(auto-detected)_ | main app file |
| `templates` | `[]` | Jinja template directories |
| `source_roots` | `[]` | extra import resolution roots |
| `env_files` | `[".env", ".env.example"]` | env key definitions |
| `settings_env_files` | `[".env", ".env.example", ".env.unittest"]` | env files checked for `BaseSettings` coverage |
| `process_env` | `false` | include server process env |
| `process_env_show_values` | `false` | show process-env values in hover |
| `client_fixtures` | `["client", "async_client"]` | pytest HTTP client fixture names |
| `env.ignore` | `[]` | env keys to suppress from diagnostics |

Feature toggles — all `true` by default except `test_unknown_paths`:

| Toggle | Default |
|---|---|
| `diagnostics` | `true` |
| `completion` | `true` |
| `hover` | `true` |
| `navigation` | `true` |
| `code_actions` | `true` |
| `code_lens` | `true` |
| `symbols` | `true` |
| `inlay_hints` | `true` |
| `document_links` | `true` |
| `test_unknown_paths` | `false` |

```toml
# fastapi-lsp.toml
entrypoint = "app/main.py"
templates = ["app/templates"]

[features]
code_lens = false

[check]
only = []
ignore = ["env/undefined-key"]
```

## CLI

```
fastapi-lsp lsp [--stdio | --tcp --address 127.0.0.1 --port 9257]
fastapi-lsp check PATH [--only CODES] [--ignore CODES] [--format text|json] [--fix]
fastapi-lsp routes [PATH] [--format text|json]
```

## Development

```bash
cargo build
cargo test
uv run --group dev pytest e2e/ -v
```

## License

MIT
