# E15 — App Config

> **Status:** Draft
>
> **Version:** 0.2   ·   **Last updated:** 2026-06-12
>
> **Purpose:** Where the server's configuration comes from — initialization options, the server's own config file, `pyproject.toml` — the schema they share, and the precedence between them.
>
> **Depends on:** [E01-architecture](E01-architecture.md)   ·   **Related:** [F05-templates](../features/F05-templates.md), [F09-env-settings](../features/F09-env-settings.md), [F17-cli](../features/F17-cli.md)

> Requirement tag: **CFG**

---

## 1. Purpose & Scope

The server aims for zero configuration: a plain FastAPI project works with no config at all. When configuration exists, one schema is read from three places, and the most session-specific source wins.

## 2. Detailed Specification

### 2.1 Sources and precedence

**REQ-CFG-04 — One schema, three sources, fixed precedence.**

Configuration merges per key, most specific source first:

1. **`InitializationOptions`** — sent by the editor in `initialize`; per-session, highest precedence.
2. **`fastapi-lsp.toml`** — the server's own file at the workspace root.
3. **`[tool.fastapi-lsp]` in `pyproject.toml`** — for projects that keep all tool config in one file.
4. Built-in defaults.

The full schema, with defaults:

```toml
# fastapi-lsp.toml  (same keys under [tool.fastapi-lsp] in pyproject.toml,
#                    same shape as JSON in InitializationOptions)
entrypoint = ""                          # path to the file holding the main app
templates = []                           # template roots, workspace-relative
env_files = [".env", ".env.example"]     # env definition files, in precedence order
process_env = false                      # consult the server's own process env (F09)

[features]                               # per-capability toggles, all true by default
diagnostics = true
completion = true
hover = true
code_actions = true
inlay_hints = true
code_lens = true

[check]                                  # defaults for the `check` subcommand (F17)
only = []                                # run only these diagnostic codes
ignore = []                              # skip these diagnostic codes
```

**Decision (resolves OQ-CFG-2)** — feature toggles are the `[features]` table: a user running this server beside a primary Python LSP can switch any capability off. A disabled capability is not advertised in the `initialize` response.

### 2.2 Template roots

**REQ-CFG-01 — Template roots resolve in a fixed order.**

1. The `templates` config key (any source per REQ-CFG-04).
2. Static detection → every `Jinja2Templates(directory=...)` whose directory is a literal, a module-level string constant, or a list literal of those (Starlette accepts a sequence of directories).
3. Fallback → a `templates/` directory at the workspace root, if it exists.

All roots are interpreted relative to the workspace root. The bookshop needs no config: source 2 finds `Jinja2Templates(directory="templates")` in `app/pages.py`.

### 2.3 Entrypoint hint

**REQ-CFG-02 — The entrypoint narrows app discovery, never replaces it.**

The `entrypoint` key (with `[tool.fastapi]`'s `entrypoint` in `pyproject.toml` honored as a final fallback, since the official VS Code extension reads it) prefers the named file's `FastAPI()`/`Starlette()` instance as *the* app for chain-root purposes when multiple apps exist. Indexing still covers the whole workspace either way.

### 2.4 Env sources

**REQ-CFG-05 — Env definition sources are configurable and code-discoverable.**

The env index ([F09](../features/F09-env-settings.md)) reads from, in order:

1. The `env_files` list — workspace-relative paths, earlier entries win on duplicate keys.
2. Files **discovered from code**: paths named in `Config(".env")`, `SettingsConfigDict(env_file=...)`, `load_dotenv(...)`, and friends — the full catalog is [F09 REQ-ENV-08](../features/F09-env-settings.md).
3. The server's **own process environment**, only when `process_env = true`. The LSP server inherits the environment the editor was launched with — a useful proxy for "what the dev shell exports", but it varies by launch method (desktop launchers export less than terminals), so it's opt-in and its values are labeled `(process)` wherever shown.

### 2.5 Robustness

**REQ-CFG-03 — Config errors degrade to defaults.**

A malformed config file or missing referenced path logs a warning (stderr, via `tracing`) and falls back to the next source. Unknown keys are ignored without complaint. Config files are watched (REQ-ARCH-12); a change re-resolves config and triggers a relink.

## 3. Edge Cases & Failure Modes

- The same key set in all three sources → InitializationOptions wins, per key (a file setting `templates` and the editor setting only `features.code_lens` merge cleanly).
- `env_files` lists a missing file → warn, skip, keep the rest.
- Two template roots contain the same relative path → first in precedence order wins.
- Multiple `pyproject.toml` (monorepo) → only the workspace-root one is read. **OQ-CFG-1** tracks per-package support.

## 4. Open Questions & Decisions

- **OQ-CFG-1** — Monorepo: per-package config support. Deferred until a real workspace needs it.
- **Decision** — OQ-CFG-2 resolved by REQ-CFG-04's `[features]` table.

## 5. Cross-References

- **Depends on:** [E01-architecture](E01-architecture.md) — config watching and relink.
- **Related:** [F05-templates](../features/F05-templates.md) — template roots; [F09-env-settings](../features/F09-env-settings.md) — env sources; [F17-cli](../features/F17-cli.md) — the `[check]` table.

## 6. Changelog

- **2026-06-12** — v0.2: full config system — three sources with per-key precedence (REQ-CFG-04), `[features]` toggles (resolves OQ-CFG-2), configurable `env_files` + opt-in `process_env` (REQ-CFG-05), `[check]` defaults. Dropped the `jinja.toml` source in favor of the unified schema.
- **2026-06-12** — Doc-verification fix: `Jinja2Templates(directory=...)` accepts a sequence of directories.
- **2026-06-12** — Added OQ-CFG-2 (feature toggles).
- **2026-06-12** — Initial draft: template-root precedence, entrypoint, degrade-to-defaults rule.
