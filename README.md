# fastapi-lsp

[![CI](https://github.com/alex-oleshkevich/fastapi-lsp/actions/workflows/ci.yml/badge.svg)](https://github.com/alex-oleshkevich/fastapi-lsp/actions/workflows/ci.yml)
[![Release](https://github.com/alex-oleshkevich/fastapi-lsp/actions/workflows/release.yml/badge.svg)](https://github.com/alex-oleshkevich/fastapi-lsp/releases)

A language server for FastAPI and Starlette. It understands routes, the `Depends()` graph, `url_for` reverse routing, Jinja templates, and env/settings — things a type checker cannot see. One Rust binary, any LSP-capable editor.

Static analysis only: the server never imports or executes your code.

## Features

| Capability | What it provides |
|---|---|
| **Diagnostics** | Path-param mismatches, duplicate/shadowed routes, unincluded routers, `Depends(fn())` anti-pattern, dependency cycles, broken `url_for` names, missing templates, undefined env keys |
| **Navigation** | Go-to-definition from test `client.get("/path")` to handler; follow `Depends()` chains in both directions; jump into template files and `.env` lines |
| **Hover** | Route card: resolved path, router chain, response model, dependencies, middleware |
| **Completions** | Route paths in test calls, route names in `url_for`, template paths, env keys |
| **Symbols** | Search `GET /api/books/{book_id}` in the symbol picker; paths resolved through all `include_router` prefixes |
| **Code lenses** | Test counts per handler, dependency usage and override counts |
| **`check` mode** | Same diagnostics as a CLI linter — pipe into CI with `fastapi-lsp check .` |

## Installation

```bash
cargo install fastapi-lsp   # from source
pip install fastapi-lsp      # pre-built binary via pip
```

Or download a release binary from the [releases page](https://github.com/alex-oleshkevich/fastapi-lsp/releases).

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

Include `html` / `htmldjango` filetypes — without them the server is never attached to template buffers, so template diagnostics and completions do not fire.

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

Helix sends hover and goto-definition to the first server that advertises the capability; diagnostics, completion, and symbols merge across servers. List `fastapi-lsp` first to make its hover and goto primary.

### Zed

```jsonc
// ~/.config/zed/settings.json
{
  "languages": {
    "Python": { "language_servers": ["fastapi-lsp", "..."] }
  },
  "lsp": {
    "fastapi-lsp": {
      "initialization_options": { "templates": ["app/templates"] }
    }
  }
}
```

## Configuration

Zero config works for standard projects. Configuration is loaded from three sources in decreasing priority: editor `InitializationOptions` › `fastapi-lsp.toml` at the workspace root › `[tool.fastapi-lsp]` in `pyproject.toml`.

### Options

| Option | Default | Description |
|---|---|---|
| `entrypoint` | _(auto-detected)_ | Path to the file that creates the `FastAPI()` app instance |
| `templates` | `[]` | Directories to scan for Jinja templates |
| `source_roots` | `[]` | Additional source roots for import resolution |
| `env_files` | `[".env", ".env.example"]` | Env files scanned for key definitions |
| `settings_env_files` | `[".env", ".env.example", ".env.unittest"]` | Env files checked for `BaseSettings` field coverage |
| `process_env` | `false` | Include the process environment when checking env keys |
| `client_fixtures` | `["client", "async_client"]` | pytest fixture names treated as HTTP test clients |
| `env.ignore` | `[]` | Env key codes to suppress (e.g. `["DJANGO_SECRET_KEY"]`) |

### Feature toggles

All features are enabled by default. Disable any individually under `[features]`:

| Feature | Default | Controls |
|---|---|---|
| `diagnostics` | `true` | All diagnostic codes |
| `completion` | `true` | Route path, `url_for`, template, env key completions |
| `hover` | `true` | Route cards, dependency summaries |
| `navigation` | `true` | Go-to-definition and references |
| `code_actions` | `true` | Quick fixes, extract-router, extract-dependency |
| `code_lens` | `true` | Test count, dependency usage, override count lenses |
| `symbols` | `true` | Workspace symbol search |
| `inlay_hints` | `true` | Inline path-param type hints |
| `document_links` | `true` | Clickable template and env file links |

### Check defaults

| Option | Default | Description |
|---|---|---|
| `check.only` | `[]` | Run only these diagnostic codes |
| `check.ignore` | `[]` | Suppress these diagnostic codes in `check` mode |

### Example

```toml
# fastapi-lsp.toml
entrypoint = "app/main.py"
templates = ["app/templates"]
env_files = [".env", ".env.example"]

[features]
code_lens = false

[check]
ignore = ["env/undefined-key"]
```

## CLI

```
fastapi-lsp lsp [--stdio | --http --address 127.0.0.1 --port 9257]
fastapi-lsp check PATH [--only CODES] [--ignore CODES] [--format text|json]
```

`check` exits non-zero when any Warning-or-worse diagnostic is found. The `--format json` flag emits NDJSON — one diagnostic object per line, suitable for scripting.

## Development

```bash
cargo build                             # build debug binary
cargo test                              # unit tests
uv run --group dev pytest e2e/ -v      # e2e tests (requires debug binary)
RUST_LOG=debug ./target/debug/fastapi-lsp lsp --stdio   # manual LSP session
```

## License

MIT
