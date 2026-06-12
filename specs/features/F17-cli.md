# F17 — CLI

> **Status:** Draft
>
> **Version:** 0.1   ·   **Last updated:** 2026-06-12
>
> **Purpose:** The binary's command-line surface: the `lsp` subcommand (stdio or HTTP transport) and the `check` subcommand — the same diagnostics engine running as a standalone linter.
>
> **Depends on:** [E01-architecture](../foundations/E01-architecture.md), [F02-diagnostics](F02-diagnostics.md)   ·   **Related:** [E15-app-config](../foundations/E15-app-config.md)

> Requirement tag: **CLI**

---

## 1. Purpose & Scope

One binary, two modes. `lsp` is the long-running server an editor spawns; `check` runs the identical scan-link-diagnose pipeline once over a path and prints what it found — CI and pre-commit get the same findings the editor shows, by construction, because both modes call the same `features::diagnostics` code.

## 2. Non-Goals / Out of Scope

- Auto-fixing from the CLI (`check --fix`) — the code-action machinery assumes an editor applying `WorkspaceEdit`s; recorded as **OQ-CLI-1**, not v1.
- Watch mode for `check` — run it again; the editor is the watch mode.

## 3. Detailed Specification

### 3.1 `fastapi-lsp lsp`

**REQ-CLI-01 — The server subcommand selects a transport.**

```
fastapi-lsp lsp --stdio                          # default; what editors spawn
fastapi-lsp lsp --http --address 127.0.0.1 --port 9257
```

`--stdio` is the default when no transport flag is given. `--http` serves the LSP JSON-RPC stream over a socket for debugging and remote-editor setups; `--address` defaults to `127.0.0.1` and `--port` to `9257`. The bare invocation `fastapi-lsp --stdio` (no subcommand) keeps working as an alias, since several editors' default configs assume that shape.

### 3.2 `fastapi-lsp check`

**REQ-CLI-02 — `check` runs the diagnostics pipeline once and reports.**

```
fastapi-lsp check PATH [--only CODES] [--ignore CODES] [--format text|json]
```

`PATH` is a file or directory; a directory is treated as the workspace root (config resolution per [E15](../foundations/E15-app-config.md) included — `[check]` table supplies default `only`/`ignore`). The run is exactly: scan → link → every [F02](F02-diagnostics.md) check → print → exit.

- `--only route/duplicate,di/cycle` — run only these codes; `--ignore env/undefined-key` — run all but these. Flags override the config table; `--only` and `--ignore` together is an error.
- Exit code `1` when any diagnostic of severity Warning or Error was emitted, `0` otherwise (Hint/Information never fail a build), `2` on usage/config errors.

**REQ-CLI-03 — Output formats.**

`text` (default) prints one finding per line in the conventional linter shape, with the related location when one exists:

```
app/routers/books.py:14:13 warning[route/duplicate] duplicate of GET /api/books/{book_id}
  ↳ related: app/routers/books.py:9:13 (the other registration)
```

`json` emits one JSON object per finding (file, range, severity, code, message, related), one per line — stable shape, scriptable.

### 3.3 Shared engine

**REQ-CLI-04 — `check` and the LSP server share one diagnostics implementation.**

`check` constructs the same `WorkspaceState`, runs the same pass 1/pass 2, and calls the same pure diagnostic functions. No check may exist in one mode and not the other; the e2e suite asserts the parity on the broken fixtures ([E17](../foundations/E17-testing.md)).

## 4. Examples & Use Cases

CI runs `fastapi-lsp check . --ignore env/undefined-key` — the env hints are noise in CI where no `.env` is checked out, but the route and dependency checks gate the merge. Locally you run `fastapi-lsp check app/routers/books.py` after a refactor and get the same three findings your editor was showing.

## 5. Edge Cases & Failure Modes

- `PATH` is a single file inside a larger project → the *workspace* is still the enclosing project root (nearest `pyproject.toml`/`.git`), so cross-file linking works; only findings located in `PATH` are printed.
- Unknown code in `--only`/`--ignore` → exit 2 with the list of valid codes; silent typos would silently skip checks.
- No FastAPI/Starlette indicators under `PATH` → exit 0 with a `no app found` note on stderr.

## 6. Open Questions & Decisions

- **OQ-CLI-1** — `check --fix` applying the deterministic quick fixes. Wants the action machinery decoupled from `WorkspaceEdit` first.

## Data Shapes & Code Map

```rust
// src/main.rs — clap derive
pub enum Cli { Lsp(LspArgs), Check(CheckArgs) }
pub struct LspArgs   { pub stdio: bool, pub http: bool, pub address: IpAddr, pub port: u16 }
pub struct CheckArgs { pub path: PathBuf, pub only: Vec<DiagCode>, pub ignore: Vec<DiagCode>,
                       pub format: OutputFormat }
pub enum OutputFormat { Text, Json }

// src/check.rs
pub fn run_check(args: CheckArgs) -> ExitCode;          // 0 clean · 1 findings ≥ Warning · 2 usage/config
pub enum CheckError { BadCode(String), NoWorkspace(PathBuf), Io(std::io::Error) }   // all map to exit 2
```

Files: `main.rs` (parsing + dispatch), `check.rs` (one-shot pipeline + printers). `DiagCode::parse` rejects unknown codes at argument-parse time ([F02](F02-diagnostics.md)'s enum is the single source).

## 7. Cross-References

- **Depends on:** [E01](../foundations/E01-architecture.md) — the shared pipeline; [F02](F02-diagnostics.md) — the codes and severities.
- **Related:** [E15](../foundations/E15-app-config.md) — the `[check]` config table; [E17](../foundations/E17-testing.md) — parity tests.

## 8. Changelog

- **2026-06-12** — Initial draft: `lsp` transports, `check` with code filters and text/json output, shared-engine rule.
