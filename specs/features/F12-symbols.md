# F12 — Symbols

> **Status:** Draft
>
> **Version:** 0.1   ·   **Last updated:** 2026-06-12
>
> **Purpose:** Routes as searchable symbols through `textDocument/documentSymbol` and `workspace/symbol` — the editor-agnostic route explorer.
>
> **Depends on:** [F01-route-index](F01-route-index.md)   ·   **Related:** [F06-starlette-routing](F06-starlette-routing.md)

> Requirement tag: **SYM**

---

## 1. Purpose & Scope

The official VS Code extension ships a custom route-explorer tree. Our equivalent costs zero editor-specific code: publish each route as a symbol, and every editor's outline and symbol picker becomes a route browser.

## 2. Non-Goals / Out of Scope

- Plain Python symbols (classes, functions) — the primary Python LSP publishes those; we add only what it can't know.

## 3. Detailed Specification

**REQ-SYM-01 — Routes are symbols named by method and resolved path.**

Document symbols and workspace symbols present each route as `GET /api/books/{book_id}`, kind `Function`, located at the handler. Starlette table entries join identically: `WEBSOCKET /ws`, and terminal mounts as `MOUNT /static` (kind `Namespace`).

**REQ-SYM-02 — Queries match path, method, and handler name.**

A workspace-symbol query for `books`, `GET`, or `get_book` all find the bookshop's routes — match against the symbol name *and* the handler name, so users can search by whichever they remember.

**REQ-SYM-03 — Unresolved routes still appear.**

A route whose chain didn't resolve shows as `GET ⟨unresolved⟩/books/{book_id}` ([F01](F01-route-index.md) REQ-ROUTE-05) — hiding it would make the symbol list lie about what the file registers.

## 4. Examples & Use Cases

You hit your editor's workspace-symbol key and type `POST` — every POST endpoint in the app, with full resolved paths, no extension UI involved.

## 5. Open Questions & Decisions

- **OQ-SYM-1** *(moved from F01 OQ-ROUTE-2)* — Also emit one symbol per router (`/api/books — 3 routes`, kind `Module`) for orientation in large apps? Decide during M1 dogfooding.

## Data Shapes & Code Map

```rust
// src/features/symbols.rs — two pure reads over route_index
pub fn document_symbols(state: &WorkspaceState, uri: &Url) -> Vec<DocumentSymbol>;
pub fn workspace_symbols(state: &WorkspaceState, query: &str) -> Vec<WorkspaceSymbol>;

fn symbol_name(r: &RouteRecord) -> String;     // "GET /api/books/{book_id}" / "MOUNT /static"
fn matches(r: &RouteRecord, query: &str) -> bool;   // path | method | handler name (REQ-SYM-02)
```

Files: `features/symbols.rs`. No state, no errors — empty index means empty list.

## 6. Cross-References

- **Depends on:** [F01](F01-route-index.md) — the route index, including F06's table-style entries.
- **Related:** [F06](F06-starlette-routing.md) — mounts and websocket symbols.

## 7. Changelog

- **2026-06-12** — Extracted from F01 §5.3 (REQ-ROUTE-06) into a capability spec; absorbed OQ-ROUTE-2 as OQ-SYM-1.
