# F13 — Navigation

> **Status:** Draft
>
> **Version:** 0.2   ·   **Last updated:** 2026-06-12
>
> **Purpose:** Everything `textDocument/definition`, `textDocument/references`, and `textDocument/documentLink` do — the clickable edges between routes, dependencies, tests, templates, and env files.
>
> **Depends on:** [F01-route-index](F01-route-index.md), [F03-dependency-graph](F03-dependency-graph.md), [F04-test-linking](F04-test-linking.md), [F05-templates](F05-templates.md), [F09-env-settings](F09-env-settings.md)   ·   **Related:** [F10-hover](F10-hover.md)

> Requirement tag: **NAV**

---

## 1. Purpose & Scope

The framework wires things together through strings and decorator arguments; this spec makes every one of those wires a clickable edge — in both directions where both directions exist.

## 2. Non-Goals / Out of Scope

- Plain name resolution (variables, imports, functions called directly) — the primary Python LSP's job (P5). We navigate only edges that exist *because of the framework*.
- Rename across these edges — deliberately unsupported for now; renaming a route path is a behavior change, not a refactor.

## 3. Detailed Specification

### 3.1 Goto definition

**REQ-NAV-01 — Route wiring is clickable.**

- `include_router` target (`books.router`) → the `APIRouter(...)` assignment.
- Router variable in a decorator (`@router.get`) → the `APIRouter(...)` assignment.
- `response_model=Book` → the model definition (from `model_index`).
- A `url_for` name string → the named handler ([F01](F01-route-index.md) §5.3).
- A `Depends(get_db)` argument → `def get_db`, in every home [F03](F03-dependency-graph.md) REQ-DI-01 recognizes.
- A client-call path string → the matched handler(s) via concrete trie lookup ([F04](F04-test-linking.md) REQ-TLINK-02); multiple matches return all, letting the editor show a picker.
- A template string → the template file at line 1 ([F05](F05-templates.md) REQ-TPL-02).
- An env key string or settings field → the `KEY=` line, `.env` first, `.env.example` when only defined there ([F09](F09-env-settings.md)).

### 3.2 Find references

**REQ-NAV-02 — References aggregate every edge kind pointing at the target.**

References on a **handler** include its client-call sites and `url_for` sites. References on a **dependency function** include every `Depends` site naming it — and its `dependency_overrides` sites ([F03](F03-dependency-graph.md)) — across the workspace. How results combine with the primary LSP's is editor-specific: Neovim appends both servers' results to the quickfix list without deduplicating, and Helix doesn't merge references across servers at all — it asks the first capable server only. [F07 §3.3](F07-editor-integration.md) documents that ordering trade-off.

### 3.3 Document links

**REQ-NAV-03 — Document links are a progressive enhancement, not a load-bearing surface.**

`textDocument/documentLink` covers recognized template strings, so they render clickable without a keybinding — in clients that consume the capability. None of the three first-class editors does today: Zed, Neovim's built-in client, and Helix all skip the request, so goto definition (REQ-NAV-01) carries the feature everywhere, and links light up for free wherever support lands later. Env keys stay goto-only even then — a `DocumentLink.target` is a bare URI and cannot address the `KEY=` line. Route-ish strings (client paths, `url_for`) stay goto-only by choice: a test file fully underlined with links is noise.

## 4. Examples & Use Cases

Ctrl-click the string in `client.get("/api/books/1")` and land in `get_book`. Find references on `get_db`: the `Depends` in `list_books`, the one inside `get_current_user` — injection sites only, no import lines mixed in. Ctrl-click `"book_list.html"` and you're in the template — where its `url_for` strings are themselves navigable ([F05 REQ-TPL-06](F05-templates.md)).

## 5. Edge Cases & Failure Modes

- Goto on an unbound `Depends` name → null (the create-dependency action in [F08](F08-code-actions.md) §3.8 is the helpful path).
- A client path matching zero routes → null, no error — it may be a deliberate 404 test.

## Data Shapes & Code Map

```rust
// src/features/goto.rs, references.rs, document_link.rs
pub fn goto(state: &WorkspaceState, uri: &Uri, pos: Position) -> Option<GotoDefinitionResponse>;
pub fn references(state: &WorkspaceState, uri: &Uri, pos: Position) -> Vec<Location>;
pub fn document_links(state: &WorkspaceState, uri: &Uri) -> Vec<DocumentLink>;

enum Edge { IncludeTarget, RouterVar, ResponseModel, UrlForName, DependsName,
            ClientPath, TemplateName, EnvKey }                            // one variant per REQ-NAV-01 bullet
fn edge_at(state: &WorkspaceState, uri: &Uri, pos: Position) -> Option<Edge>;
```

Files: `features/goto.rs`, `features/references.rs`, `features/document_link.rs` — all dispatch through one shared `edge_at` so goto and references can never disagree about what's under the cursor.

## 6. Cross-References

- **Depends on:** the five domain specs in the header — each owns the index behind one edge kind.
- **Related:** [F10](F10-hover.md) — the read-only counterpart; [F08](F08-code-actions.md) — actions offered where navigation dead-ends.

## 7. Changelog

- **2026-06-12** — v0.2 review pass: REQ-NAV-03 reworded to progressive enhancement (no first-class editor consumes documentLink today; env links can't target a line); REQ-NAV-02 merge claim corrected per editor (Neovim quickfix doesn't dedupe, Helix doesn't merge) and override sites included in dependency references; `Uri` type in data shapes.
- **2026-06-12** — Extracted from F01 §5.6/§5.7 (REQ-ROUTE-09, goto part of REQ-ROUTE-11), F03 §3.3 (goto/references of REQ-DI-03), F04 §3.3 (navigation part of REQ-TLINK-03), F05 §3.3 (REQ-TPL-03), F09 §3.3 (goto part of REQ-ENV-05) into a capability spec.
