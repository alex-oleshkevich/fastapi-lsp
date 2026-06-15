# F17 — CLI

> **Status:** Draft
>
> **Version:** 0.2   ·   **Last updated:** 2026-06-12
>
> **Purpose:** The binary's command-line surface: the `lsp` subcommand (stdio or TCP transport) and the `check` subcommand — the same diagnostics engine running as a standalone linter.
>
> **Depends on:** [E01-architecture](../foundations/E01-architecture.md), [F02-diagnostics](F02-diagnostics.md)   ·   **Related:** [E15-app-config](../foundations/E15-app-config.md)

> Requirement tag: **CLI**

---

## 1. Purpose & Scope

One binary, two modes. `lsp` is the long-running server an editor spawns; `check` runs the identical scan-link-diagnose pipeline once over a path and prints what it found — CI and pre-commit get the same findings the editor shows, by construction, because both modes call the same `features::diagnostics` code.

## 2. Non-Goals / Out of Scope

- Watch mode for `check` — run it again; the editor is the watch mode.

## 3. Detailed Specification

### 3.1 `fastapi-lsp lsp`

**REQ-CLI-01 — The server subcommand selects a transport.**

```
fastapi-lsp lsp --stdio                          # default; what editors spawn
fastapi-lsp lsp --tcp --address 127.0.0.1 --port 9257
```

`--stdio` is the default when no transport flag is given. `--tcp` serves the LSP JSON-RPC stream over a TCP socket — `tower-lsp-server` supports this directly — for debugging and remote-editor setups; `--address` defaults to `127.0.0.1` and `--port` to `9257`. (LSP defines no HTTP transport, so there is none here.) The bare invocation `fastapi-lsp --stdio` (no subcommand) keeps working as an alias, since several editors' default configs assume that shape.

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

### 3.3 `fastapi-lsp check --fix`

**REQ-CLI-05 — `--fix` applies deterministic quick fixes in-place.**

```
fastapi-lsp check PATH --fix [--only CODES] [--ignore CODES]
```

Only the subset of quick fixes that are deterministic (single right answer, no ambiguity) are applied: rename a mismatched path param, add a missing `/{param}` segment, rewrite `Depends(fn())` to `Depends(fn)`. Fixes that require a choice (e.g. which param name to keep when two conflict) are not applied. A dry run of the diagnostics pipeline runs first; only fixable findings trigger edits.

Exit codes match `check` (`0` = clean after fixes, `1` = unfixable warnings remain, `2` = usage error). The fix logic lives in a shared layer (`src/fixes.rs`) that produces plain file edits — no `WorkspaceEdit` envelope — so the same code can be called from the LSP code-action path too. The `--format json` flag in `--fix` mode emits a per-finding object with an `applied: bool` field.

### 3.4 `fastapi-lsp routes`

**REQ-CLI-06 — `routes` prints the resolved route table.**

```
fastapi-lsp routes [PATH] [--format text|json]
```

For each route in the index, one row: HTTP method, fully-resolved path, handler function name, and source location (`file:line`). Unresolved routes show `⟨unresolved⟩` as the path prefix. `text` (default) is a fixed-width table; `json` emits one object per route per line.

```
GET     /api/books               list_books       app/routers/books.py:8
GET     /api/books/{book_id}     get_book         app/routers/books.py:14
POST    /api/books               create_book      app/routers/books.py:22
```

Exit `0` always (it's a query, not a lint). Error `2` when `PATH` resolves no workspace.

### 3.5 Shared engine

**REQ-CLI-04 — `check`, `check --fix`, and the LSP server share one diagnostics implementation.**

`check` constructs the same `WorkspaceState`, runs the same pass 1/pass 2, and calls the same pure diagnostic functions. No check may exist in one mode and not the other; the e2e suite asserts the parity on the broken fixtures ([E17](../foundations/E17-testing.md)). Fix logic lives in `src/fixes.rs` — plain `(PathBuf, TextEdit)` pairs — shared between `check --fix` and the LSP code-action path.

## 4. Examples & Use Cases

CI runs `fastapi-lsp check . --ignore env/undefined-key` — the env hints are noise in CI where no `.env` is checked out, but the route and dependency checks gate the merge. Locally you run `fastapi-lsp check app/routers/books.py` after a refactor and get the same three findings your editor was showing.

## 5. Edge Cases & Failure Modes

- `PATH` is a single file inside a larger project → the *workspace* is still the enclosing project root (nearest `pyproject.toml`/`.git`), so cross-file linking works; only findings located in `PATH` are printed.
- Unknown code in `--only`/`--ignore` → exit 2 with the list of valid codes; silent typos would silently skip checks.
- No FastAPI/Starlette indicators under `PATH` → exit 0 with a `no app found` note on stderr.

## 6. Open Questions & Decisions

- ~~**OQ-CLI-1**~~ — **Decision:** Implement `check --fix` (REQ-CLI-05, shared engine REQ-CLI-04). Fix logic extracted to `src/fixes.rs` shared with the LSP code-action path.
- ~~**OQ-CLI-2**~~ — **Decision:** Implement `fastapi-lsp routes` (REQ-CLI-06). Near-zero marginal cost over the existing route index.

## Data Shapes & Code Map

```rust
// src/main.rs — clap derive
pub enum Cli { Lsp(LspArgs), Check(CheckArgs), Routes(RoutesArgs) }
pub struct LspArgs    { pub stdio: bool, pub tcp: bool, pub address: IpAddr, pub port: u16 }
pub struct CheckArgs  { pub path: PathBuf, pub only: Vec<DiagCode>, pub ignore: Vec<DiagCode>,
                        pub format: OutputFormat, pub fix: bool }
pub struct RoutesArgs { pub path: Option<PathBuf>, pub format: OutputFormat }
pub enum OutputFormat { Text, Json }

// src/check.rs — also src/fixes.rs (shared fix logic, called from check --fix and LSP code actions)
pub fn run_check(args: CheckArgs) -> ExitCode;          // 0 clean · 1 findings ≥ Warning · 2 usage/config
pub enum CheckError { BadCode(String), NoWorkspace(PathBuf), Io(std::io::Error) }   // all map to exit 2
```

Files: `main.rs` (parsing + dispatch), `check.rs` (one-shot pipeline + printers). `DiagCode::parse` rejects unknown codes at argument-parse time ([F02](F02-diagnostics.md)'s enum is the single source).

## 7. Cross-References

- **Depends on:** [E01](../foundations/E01-architecture.md) — the shared pipeline; [F02](F02-diagnostics.md) — the codes and severities.
- **Related:** [E15](../foundations/E15-app-config.md) — the `[check]` config table; [E17](../foundations/E17-testing.md) — parity tests.

## 8. Changelog

- **2026-06-12** — v0.2: renamed `--http` to `--tcp` — LSP JSON-RPC over a TCP socket; there is no HTTP transport. Added OQ-CLI-2: a `fastapi-lsp routes` route-table subcommand.
- **2026-06-12** — Initial draft: `lsp` transports, `check` with code filters and text/json output, shared-engine rule.
