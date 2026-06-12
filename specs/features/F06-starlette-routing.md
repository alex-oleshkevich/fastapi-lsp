# F06 — Starlette Routing

> **Status:** Draft
>
> **Version:** 0.1   ·   **Last updated:** 2026-06-12
>
> **Purpose:** Extracting table-style Starlette routing — `Route`, `Mount`, `WebSocketRoute` — into the same route index, so every navigation, diagnostic, and test-linking feature works on raw Starlette apps unchanged.
>
> **Depends on:** [F01-route-index](F01-route-index.md), [E07-data-model](../foundations/E07-data-model.md)   ·   **Related:** [F02-diagnostics](F02-diagnostics.md), [F04-test-linking](F04-test-linking.md)

> Requirement tag: **STAR**

---

## 1. Purpose & Scope

Starlette apps declare routes as data, not decorators: a list of `Route(...)` objects handed to the constructor. This spec adds that second registration style as a pass-1 extraction concern only — everything downstream of `FileFacts.routes` is shared with F01 by construction.

This spec covers:

- Extraction of `Route`, `WebSocketRoute`, and `Mount` (including `Mount`-of-app and `StaticFiles`)
- How table entries map onto the F01 route facts and chain model

## 2. Non-Goals / Out of Scope

- New features. F06 adds inputs, not outputs — symbols, hover, goto, diagnostics, and test linking come from F01–F04 for free.
- Starlette middleware, exception handlers, and lifespan — not routing.
- `app.add_route(...)` imperative registration — rare; recorded as OQ-STAR-1.

## 3. Background & Rationale

FastAPI *is* Starlette underneath, and real codebases mix the styles: a FastAPI app mounting a bare Starlette admin app, a `Mount("/static", StaticFiles(...))` next to decorated routes. Covering the table style closes that gap and makes the server honest about its name claim — FastAPI *and* Starlette.

## 4. Detailed Specification

### 4.1 Recognized forms

**REQ-STAR-01 — Table entries become route facts.**

Pass 1 extracts, from any list assigned to a `routes=` kwarg of `Starlette(...)`/`FastAPI(...)` or bound to a module-level name that flows into one:

- `Route(<path>, <endpoint>, methods=[...], name=...)` → one route fact per method (default `["GET"]`), handler = the endpoint expression resolved like an include target (import-aware). Starlette adds an implicit `HEAD` to every `GET` route; the index records only declared methods — surfacing the implicit HEAD would double the symbol list for zero information.
- `WebSocketRoute(<path>, <endpoint>)` → method `WEBSOCKET`.
- `Mount(<path>, routes=[...])` → a chain link contributing `<path>` as prefix to the nested list.
- `Mount(<path>, app=<other_app>)` → a chain link into the other app's routes when `<other_app>` resolves to a workspace `Starlette`/`FastAPI` instance; a terminal "mounted app" record otherwise (e.g. `StaticFiles` — indexed for hover/symbols as `MOUNT /static`, no handler).

**REQ-STAR-02 — Endpoint classes count as handlers.**

An endpoint that resolves to a class (Starlette's `HTTPEndpoint` style) anchors the route at the class definition; its `get`/`post` methods are not separately indexed in v1.

### 4.2 Chain semantics

**REQ-STAR-03 — Mounts are includes.**

A `Mount` is to the table style what `include_router(prefix=...)` is to the decorator style: one `ChainLink` carrying a prefix. The resolved path of a route nested two mounts deep is computed by exactly the F01 algorithm (REQ-ROUTE-04), and unresolved mount paths degrade by exactly REQ-ROUTE-05. Nothing in `linking.rs` branches on style.

## 5. Examples & Use Cases

The `health/` fixture declares `Starlette(routes=[Route("/health", health), Mount("/static", app=StaticFiles(directory="static"))])`. After indexing: workspace symbols show `GET /health` and `MOUNT /static`; hover on `health` shows its route card; `client.get("/health")` in its tests links to the handler — none of which needed F06-specific feature code.

## 6. Edge Cases & Failure Modes

- Routes list built by concatenation (`routes = api_routes + page_routes`) → followed when both operands are module-level list literals; otherwise the unknown part is dropped silently (P4) while known entries still index.
- `Route` with a lambda endpoint → fact dropped, same as F01's lambda rule.
- A FastAPI app passed `routes=[...]` *and* decorator routes → both index; duplicates between them are caught by the ordinary `route/duplicate` check.
- Mount of a third-party ASGI app (`Mount("/metrics", app=make_asgi_app())`) → terminal mount record, no recursion.

## 7. Open Questions & Decisions

- **OQ-STAR-1** — `app.add_route(...)` / `app.mount(...)` imperative forms: extract or ignore? `app.mount` is common enough that it likely joins REQ-STAR-01 during M6; decide from fixture evidence.

## Data Shapes & Code Map

Table entries normalize into F01's `RouteFact`/`ChainLink` shapes at extraction time — by design there are no F06-only record types downstream. The only new shapes are the table-side intermediates:

```rust
// src/parsing/routes.rs — table-style intermediates
pub enum TableEntry { Route(RouteFact), WebSocket(RouteFact),
                      MountRoutes { prefix: PathValue, entries: Vec<TableEntry>, name: Option<String> },
                      MountApp    { prefix: PathValue, target: MountTarget, name: Option<String> } }
pub enum MountTarget { WorkspaceApp(DottedName), Terminal(String) }      // Terminal: "StaticFiles(...)"
```

Files: `parsing/routes.rs` (one extraction module for both styles — REQ-STAR-03's "nothing branches on style" starts here).

## 8. Cross-References

- **Depends on:** [F01](F01-route-index.md) — chain algorithm and degradation rules; [E07](../foundations/E07-data-model.md) — the shared fact shapes.
- **Related:** [F02](F02-diagnostics.md), [F04](F04-test-linking.md) — downstream consumers that light up for free; [E17](../foundations/E17-testing.md) — the `health/` fixture.

## 9. Changelog

- **2026-06-12** — Doc-verification fix: implicit-HEAD-on-GET recorded; index keeps declared methods only.
- **2026-06-12** — Initial draft: table-form extraction, mounts-as-includes, endpoint classes.
