# F11 — Completion

> **Status:** Draft
>
> **Version:** 0.2   ·   **Last updated:** 2026-06-12
>
> **Purpose:** Everything `textDocument/completion` offers. All of it is string-position completion — values inside string literals that only the framework indices can know.
>
> **Depends on:** [F01-route-index](F01-route-index.md), [F04-test-linking](F04-test-linking.md), [F05-templates](F05-templates.md), [F09-env-settings](F09-env-settings.md)   ·   **Related:** [constitution](../constitution.md)

> Requirement tag: **CPL**

---

## 1. Purpose & Scope

Pylance completes Python; we complete what Python's type system can't see. Almost everything below triggers only inside string literals of recognized calls; the one non-string surface (middleware kwargs, REQ-CPL-06) earns its place because a `**options` indirection hides the signature from type checkers too. That's the P5 test in both cases: we only complete where the primary LSP is structurally blind.

## 2. Non-Goals / Out of Scope

- Attribute, kwarg, and import completion — type-driven, owned by the user's primary Python LSP (P5). Exception: kwargs hidden behind `**kwargs` forwarding, where no type checker can help — see REQ-CPL-06.
- Completion inside template files — a dedicated Jinja server's job, except route names inside template `url_for(` strings ([F05 REQ-TPL-06](F05-templates.md)), which complete from our route index like any other `url_for` site.

## 3. Detailed Specification

### 3.1 Dispatch

**REQ-CPL-01 — String-position gating.**

The provider walks up from the cursor via `find_enclosing_call`; if the cursor is not inside a string-literal argument of a recognized call shape, the response is empty. Snippet-style insertions are used only when the client advertised `completionItem.snippetSupport`; plain text otherwise.

### 3.2 The four surfaces

**REQ-CPL-02 — Route paths in client calls.**

Inside the path string of a recognized client call ([F04](F04-test-linking.md) REQ-TLINK-01, including `websocket_connect` with verb `WEBSOCKET`), every resolved route path for the call's verb — `client.get("` offers `/api/books/`, `/api/books/{book_id}`. Param segments insert as placeholder snippets (`/api/books/${1:book_id}`).

**REQ-CPL-03 — Route names in `url_for`.**

Inside the first string argument of `url_for` ([F01](F01-route-index.md) §5.3), every indexed route name with method and resolved path as the detail (`get_book — GET /api/books/{book_id}`), and the route's remaining path params appended as snippet kwargs (`get_book", book_id=$1`).

**REQ-CPL-04 — Template paths, directory-aware.**

Inside a recognized template string ([F05](F05-templates.md) REQ-TPL-01), index entries filtered by the typed prefix. Directories complete as `admin/` (kind `Folder`) and re-trigger so deep trees complete one level at a time; files complete as `book_list.html` (kind `File`).

**REQ-CPL-05 — Env keys.**

Inside a recognized env lookup string ([F09](F09-env-settings.md) REQ-ENV-02), the union of `.env` keys, `.env.example` keys, and resolved settings-field keys, with the value as the detail — masked under the same rule as hover ([F10](F10-hover.md) REQ-HOV-05).

**REQ-CPL-06 — Middleware kwargs.**

At a recognized middleware registration site ([F16](F16-middleware.md)), after the class argument, the resolved signature's kwargs complete as `allow_origins=` with the parameter's annotation and default as the detail. Kwargs already present in the call are filtered out. This is the spec's only non-string completion surface; its justification lives in [F16 §1](F16-middleware.md).

### 3.3 Mechanics

**REQ-CPL-07 — Trigger characters and explicit edit ranges.**

Editors auto-invoke completion on identifier characters or on characters the server declares — and every surface above lives inside a string literal, where no identifier character ever fires. The server therefore advertises `triggerCharacters: ["\"", "'", "/", ","]`: the quotes open each surface as it's typed, `/` re-triggers path and template completion one segment at a time, and `,` fires the kwarg surface (REQ-CPL-06, also reachable by manual invoke). Without these, none of the moments in §4 ever happens.

Replacement ranges are the other silent killer. Clients left to guess fall back to word boundaries, and `/`, `{`, and `.` all break words — an item like `/api/books/{book_id}` typed against `"/api/b` gets filtered out or inserted doubled. Every in-string item therefore carries an explicit `textEdit` spanning from just after the opening quote to the cursor, with a `filterText` matching that span (ranges follow the encoding rules of [E01 REQ-ARCH-09](../foundations/E01-architecture.md)). Directory-level template results return `isIncomplete: true`, so the client re-queries as the user descends.

## 4. Examples & Use Cases

In a test you type `client.get("` and pick `/api/books/{book_id}`; the param drops in as a tab-stop. In a handler you type `templates.TemplateResponse("` and walk `admin/` → `books/` → `list.html` one directory at a time. In a script, `os.getenv("` offers `SMTP_HOST` that so far exists only in `.env.example`.

## 5. Edge Cases & Failure Modes

- Cursor in an f-string → not a plain literal; no completion (consistent with P4-style honesty about interpolation).
- Empty index (fresh scan still running) → empty result now, correct result after the scan; never an error.

## Data Shapes & Code Map

```rust
// src/features/completion.rs
pub fn complete(state: &WorkspaceState, uri: &Uri, pos: Position, caps: &ClientCaps) -> Vec<CompletionItem>;

enum Surface { ClientPath { verb: Method }, RouteName, TemplatePath { prefix: String },
               EnvKey, MiddlewareKwarg { class: String } }
fn classify(state: &WorkspaceState, uri: &Uri, pos: Position) -> Option<Surface>;   // REQ-CPL-01 gate
```

Files: `features/completion.rs` — `classify` is the only place that walks the tree; each surface then renders from its index. `caps.snippet_support` switches snippet vs plain-text insertions once, at render time.

## 6. Cross-References

- **Depends on:** [F01](F01-route-index.md), [F04](F04-test-linking.md), [F05](F05-templates.md), [F09](F09-env-settings.md) — the indices behind each surface.
- **Related:** [constitution](../constitution.md) — P5 is the boundary this spec is built around.

## 7. Changelog

- **2026-06-12** — v0.2 review pass: REQ-CPL-07 — trigger characters (`"` `'` `/` `,`), explicit `textEdit` + `filterText` on every in-string item, `isIncomplete` for directory-level template results; `websocket_connect` noted in REQ-CPL-02; `Uri` type in data shapes.
- **2026-06-12** — Extracted from F04 §3.3 (REQ-TLINK-04), F01 §5.7 (completion part of REQ-ROUTE-11), F05 §3.3 (REQ-TPL-04), F09 §3.3 (completion part of REQ-ENV-05) into a capability spec.
