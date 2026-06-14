# Overview — fastapi-lsp

> **Status:** Draft
>
> **Version:** 0.2   ·   **Last updated:** 2026-06-12
>
> **Purpose:** What fastapi-lsp is, who it's for, and what it does — in plain language. Start here if you're new.
>
> **Related:** [roadmap](roadmap.md), [E01-architecture](foundations/E01-architecture.md)

---

## What it is

fastapi-lsp is a language server that understands FastAPI and Starlette the way a framework expert does. It's a single Rust binary speaking the Language Server Protocol over stdio, so it works in any LSP-capable editor — Zed, Neovim, and Helix are the first-class targets.

A type checker sees `@app.get("/books/{book_id}")` as a decorator taking a string. This server sees a route: it knows the final URL once router prefixes are applied, which function parameters bind to the path, which dependencies the handler pulls in, and which tests call it.

## Why it exists

FastAPI's official editor tooling is a VS Code-only TypeScript extension. Everyone else gets nothing framework-aware. And even in VS Code, the official extension stops at navigation — it doesn't diagnose route conflicts, path-parameter mismatches, or `Depends()` footguns, because a type checker can't see into string literals and decorator wiring.

fastapi-lsp fills both gaps: framework intelligence, in every editor, as one LSP.

## What it does

Each area below is a feature spec; this is the five-second version.

| Area | What you get | Spec |
|---|---|---|
| Route index & navigation | Routes as searchable symbols (`GET /api/books/{book_id}`), hover with the resolved path and router chain, inlay hints, goto definition through `include_router` | [F01](features/F01-route-index.md) |
| Diagnostics | Path-param mismatches, duplicate and shadowed routes, `Depends(fn())` called instead of referenced | [F02](features/F02-diagnostics.md) |
| Dependency graph | Goto and find-references through `Depends()` chains, cycle detection, usage counts on hover | [F03](features/F03-dependency-graph.md) |
| Test linking | Jump from `client.get("/api/books/1")` to the handler it hits; CodeLens showing test references; path completion in client calls | [F04](features/F04-test-linking.md) |
| Templates | Click-to-template on `TemplateResponse("book_list.html")`, template-name completion, missing-template diagnostics | [F05](features/F05-templates.md) |
| Raw Starlette | The same features for table-style `Route(...)` / `Mount(...)` apps | [F06](features/F06-starlette-routing.md) |
| Editor packaging | Zed extension, Neovim/Helix config snippets, Arch package | [F07](features/F07-editor-integration.md) |
| Code actions | Quick fixes for the diagnostics, plus refactors: extract a named dependency, create a missing model, extract a router | [F08](features/F08-code-actions.md) |
| Env & settings | Hover, completion, and goto for env keys backed by your `.env` files; `BaseSettings` fields bind to their keys; undefined keys get flagged | [F09](features/F09-env-settings.md) |
| Middleware | Every registration style recognized, kwarg completion on stock and workspace middleware, each route's applied chain on hover | [F16](features/F16-middleware.md) |
| CLI check mode | `fastapi-lsp check` runs the same diagnostics as a CI-friendly linter — no editor required | [F17](features/F17-cli.md) |

These rows are the domain areas; how each one actually surfaces in your editor — hover cards, completions, symbols, navigation, inlay hints, code lenses — is owned by the capability specs [F10](features/F10-hover.md)–[F15](features/F15-code-lens.md).

## What it isn't

- **Not a type checker.** Pylance/ty own types; this server owns framework semantics (per constitution P5).
- **Not a runtime tool.** It never runs your app, generates OpenAPI specs, or executes endpoints (per P1).
- **Not a VS Code extension — by choice, for now.** v1 deliberately focuses on the editors with no framework-aware tooling at all; VS Code already gets basic navigation from the official extension, so the marginal value there is lowest. Revisiting it is recorded as an open question, not a permanent exclusion.

## How it works, in one paragraph

The server scans the workspace with tree-sitter, extracting per-file facts: route decorators, `APIRouter` declarations, `include_router` calls, `Depends()` references, template usages. A second, debounced pass links those facts into workspace-level graphs — resolving each route's final path through its router chain, wiring dependencies to definitions, matching test calls to routes. Every LSP feature is then a pure lookup into those indices. The full story is in [E01-architecture](foundations/E01-architecture.md).

## Changelog

- **2026-06-12** — v0.2: "What it does" now covers code actions (F08), env & settings (F09), middleware (F16), and the CLI check mode (F17), with a pointer to the capability specs F10–F15; the VS Code stance reworded as a deliberate v1 focus on underserved editors, with the revisit recorded as an open question.
- **2026-06-12** — Initial overview.
