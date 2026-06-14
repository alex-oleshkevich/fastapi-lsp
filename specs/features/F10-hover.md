# F10 — Hover

> **Status:** Draft
>
> **Version:** 0.2   ·   **Last updated:** 2026-06-12
>
> **Purpose:** Everything `textDocument/hover` shows — the route card, dependency usage, include summaries, and env values — composed from the indices the domain specs build.
>
> **Depends on:** [F01-route-index](F01-route-index.md), [F03-dependency-graph](F03-dependency-graph.md), [F09-env-settings](F09-env-settings.md)   ·   **Related:** [F11-completion](F11-completion.md), [F13-navigation](F13-navigation.md)

> Requirement tag: **HOV**

---

## 1. Purpose & Scope

One hover provider, dispatching on what's under the cursor, answering from the indices. Hover is the server's "show me what the framework will do here" surface — it never repeats what the code already says.

## 2. Non-Goals / Out of Scope

- Type information of any kind — Pylance/ty's hover already shows it (P5); ours composes alongside it in editors that merge multiple servers' hovers. Helix does not merge hover — it asks the first capable server only, so whichever server is listed first wins; [F07](F07-editor-integration.md) §3.3 documents the ordering trade-off.
- Hover inside template files — a dedicated Jinja language server's territory (except the `url_for` sites of [F05 REQ-TPL-06](F05-templates.md), which only our route index can answer).

## 3. Detailed Specification

### 3.1 Dispatch

**REQ-HOV-01 — One provider, dispatched by cursor context.**

The hover feature resolves the cursor to a node via the parse tree, then dispatches: handler `def`/decorator → route card; `include_router` call → include summary; dependency function `def` → dependency card; recognized env key string or settings field → env card. No match returns null quickly — most hovers in a file are not ours.

### 3.2 The route card

**REQ-HOV-02 — Hovering a handler shows the route card.**

Hovering anywhere on a handler's `def` line (or its decorator) returns markdown:

```markdown
**GET** `/api/books/{book_id}`

- chain: `app` → include `/api` → `books.router` `/books`
- response model: `Book`
- dependencies: `get_db`
- path params: `book_id`
- middleware: `CORSMiddleware` → `TimingMiddleware`
```

Lines without a value are omitted. The middleware line lists the route's *applied* chain in execution order — and execution order is not source order. `add_middleware` registrations render in reverse source order (Starlette prepends each one, so the last call is outermost and runs first), followed by `middleware=[...]` constructor lists in list order. [F16](F16-middleware.md) REQ-MW-04 owns that rule; the card just renders the chain it stored. Unresolved routes show the longest-known suffix marked `⟨unresolved⟩` (per [F01](F01-route-index.md) REQ-ROUTE-05). Starlette table routes get the same card; a terminal mount renders:

```markdown
**MOUNT** `/static` — `StaticFiles(directory="static")`
```

**REQ-HOV-03 — Hovering an `include_router` call summarizes the target.**

```markdown
**router** `books.router` — 3 routes under `/api/books`

`GET /` · `GET /{book_id}` · `POST /`
```

The route list caps at 10; beyond that, `… and N more`.

### 3.3 The dependency card

**REQ-HOV-04 — Hovering a dependency function shows its place in the graph.**

On the `def` line of an indexed dependency, the card shows both directions:

```markdown
**dependency** `get_db` — used by 2 routes, 1 dependency

- used by: `list_books` (route) · `get_book` (route) · `get_current_user` (dependency)
- uses: —
- overridden in: `tests/conftest.py`
```

Direct edges only ([F03](F03-dependency-graph.md) OQ-DI-1 tracks transitive display). The override line appears only when [F03](F03-dependency-graph.md) recorded `dependency_overrides` facts for the function — worth knowing before you debug why a test never hits the real database.

### 3.4 The env card

**REQ-HOV-05 — Hovering an env key shows the value, masked when it looks secret.**

On a recognized env key string or a settings field definition:

```markdown
`API_TIMEOUT` = `30`

defined in: `.env:12` · `.env.example:8`
```

A missing key shows `[not in workspace env files]`. A value renders masked when its key contains any of `secret`, `token`, `password`, `key`, `credential` — a case-insensitive substring check, no regex involved:

```markdown
`MAIL_PASSWORD` = `••••••`

defined in: `.env:14`
```

Hovers end up in screen shares; the file is one click away via [F13](F13-navigation.md).

## 4. Examples & Use Cases

Hover `get_book`: the card shows the chain through `main.py`'s include — the resolved path the decorator alone can't tell you. Hover `get_db`'s `def`: two direct users. Hover the `database_url` settings field: the live `.env` value with its line number.

## 5. Edge Cases & Failure Modes

- Cursor on a decorator of an unindexed function (lambda rule) → null, not an empty card.
- Two hovers apply (a handler that is also a dependency) → one merged card, route section first.

## Data Shapes & Code Map

```rust
// src/features/hover.rs — one pure function, one card enum
pub fn hover(state: &WorkspaceState, uri: &Uri, pos: Position) -> Option<Hover>;

enum Card<'a> { Route(&'a RouteRecord), Router { decl: &'a RouterDecl, routes: Vec<&'a RouteRecord> },
                Dependency { node: NodeId }, Env { key: &'a str, entry: Option<&'a EnvEntry> },
                Mount(&'a RouteRecord) }
impl Card<'_> { fn render(&self) -> MarkupContent }                      // all markdown lives here
```

Files: `features/hover.rs`. Rendering is centralized in `Card::render` so card formats stay consistent and testable as plain string assertions. The `Card<'a>` borrows are sound only because they point into the immutable pass-2 snapshot: `hover` loads one `Arc<Linked>` ([E07](../foundations/E07-data-model.md)) and every reference lives inside it — never into a `DashMap` guard, which couldn't be held across the render.

## 6. Cross-References

- **Depends on:** [F01](F01-route-index.md) — `RouteRecord` quoted verbatim; [F03](F03-dependency-graph.md) — adjacency for the dependency card; [F09](F09-env-settings.md) — env index and masking rationale.
- **Related:** [F13](F13-navigation.md) — the goto counterpart of every card.

## 7. Changelog

- **2026-06-12** — v0.2 review pass: middleware line states the reverse-source-order rule, owned by F16; dependency card gains the override line from F03's `dependency_overrides` facts; masking restated as a case-insensitive substring list, no regex; `Card` borrows pinned to the `Arc<Linked>` snapshot; Helix no-hover-merge hedge with F07 §3.3 pointer.
- **2026-06-12** — Rendered popover examples added to every card; route card gains the applied-middleware line (chain from [F16](F16-middleware.md)).
- **2026-06-12** — Extracted from F01 §5.4 (REQ-ROUTE-07), F03 §3.3 (hover part of REQ-DI-03), F09 §3.3 (REQ-ENV-04) into a capability spec.
