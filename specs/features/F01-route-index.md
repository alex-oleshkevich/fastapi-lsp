# F01 — Route Index & Navigation

> **Status:** Draft
>
> **Version:** 0.1   ·   **Last updated:** 2026-06-12
>
> **Purpose:** The route index — extracting every route, resolving its final path through the router graph — and the navigation features built on it: symbols, hover, inlay hints, goto definition.
>
> **Depends on:** [E01-architecture](../foundations/E01-architecture.md), [E07-data-model](../foundations/E07-data-model.md)   ·   **Related:** [F02-diagnostics](F02-diagnostics.md), [F06-starlette-routing](F06-starlette-routing.md)

> Requirement tag: **ROUTE**

---

## 1. Purpose & Scope

This is the foundation feature: it teaches the server what a route *is*. Everything else in the suite reads the index this spec defines.

This spec covers:

- Pass-1 extraction of route decorators, `APIRouter` declarations, and `include_router` calls
- Pass-2 resolution of each route's final path through its router chain
- Document symbols, workspace symbols, hover, inlay hints, and goto definition

## 2. Non-Goals / Out of Scope

- Diagnostics on the index — owned by [F02](F02-diagnostics.md).
- Table-style `Route(...)`/`Mount(...)` extraction — owned by [F06](F06-starlette-routing.md), feeding the same index.
- Anything type-level about handler signatures (constitution P5).

## 3. Background & Rationale

A route's URL doesn't exist anywhere in the source. `get_book`'s decorator says `/{book_id}`; the real path `/api/books/{book_id}` only emerges by composing three files' worth of wiring. Resolving that composition statically is the core value of this server — it's exactly what generic Python tooling can't do.

## 4. Concepts & Definitions

Route, router chain, resolved path, and Unresolved are canonical in the [glossary](../glossary.md).

## 5. Detailed Specification

### 5.1 Extraction (pass 1)

**REQ-ROUTE-01 — Recognized registration forms.**

A route fact is extracted from a decorator of the form `@<obj>.<method>(<path>, **kwargs)` where `<method>` is one of `get`, `post`, `put`, `delete`, `patch`, `options`, `head`, `trace`, or `websocket`, and from `@<obj>.api_route(<path>, methods=[...])`. `<obj>` is recorded as written (`app`, `router`, `admin_router`) for chain resolution. The kwargs captured: `response_model`, `status_code`, `dependencies`, `tags`, `name`, `include_in_schema`.

Path parameters use Starlette's converter syntax — `{name}` or `{name:converter}`, converters `str` (default), `int`, `float`, `path`, `uuid`. The parameter *name* is the part before the colon; every check and feature that touches params (mismatch diagnostics, hover, `url_for` kwargs) compares names with the converter stripped. The converter is kept on the `PathParam` ([E07](../foundations/E07-data-model.md)) — `{p:path}` changes trie matching, since it spans multiple segments.

**REQ-ROUTE-02 — Router and include facts.**

`<name> = APIRouter(prefix=..., tags=..., dependencies=...)` produces a router fact. `<obj>.include_router(<target>, prefix=...)` produces an include fact, recording the target expression (`books.router` or bare `router`) and the prefix.

**REQ-ROUTE-03 — Prefix and path values resolve literals and module constants only.**

A path or prefix value is resolved if it is a string literal, or a name bound at module level to a string literal (a simple constant like `PREFIX = "/api"`). f-strings, concatenation, function calls, and imported constants are stored as `Unresolved` — never guessed (P4).

### 5.2 Linking (pass 2)

**REQ-ROUTE-04 — Chain resolution walks include edges to an app root.**

For each route fact, the linker finds its router, then follows include edges upward: which include call targets this router, on which object, with what prefix — repeating until it reaches a `FastAPI()`/`Starlette()` app or runs out of edges. Include targets like `books.router` are resolved through the including file's imports (`from app.routers import books` → the `router` in `app/routers/books.py`).

The resolved path is the concatenation of every prefix on the chain plus the decorator path, with slashes normalized (no doubled `//`, no trailing slash except root). FastAPI itself rejects prefixes ending in `/` at startup; the linker normalizes them anyway so navigation still works on code that hasn't been run yet.

**REQ-ROUTE-05 — Partial chains stay useful.**

A route whose chain never reaches an app (router not yet included) or crosses an `Unresolved` prefix gets `resolved_path: Unresolved` but keeps everything else — it still appears in document symbols and hover, marked `⟨unresolved⟩/books/{book_id}` with the longest-known suffix. Multiple includes of one router (mounted twice) yield one record per mount point.

### 5.3 Route names and `url_for` recognition

`url_for` is reverse routing — a string naming a route instead of a path. The index makes those names and call sites first-class.

**REQ-ROUTE-10 — Every route has a name; names are indexed.**

A route's name is its `name=` kwarg when present, else the handler's function name — Starlette's own rule. Names are prefix-independent, so even `Unresolved` routes contribute to the name index ([E07](../foundations/E07-data-model.md) `route_names`).

A *named* `Mount` namespaces the names beneath it: a route `dashboard` under `Mount("/admin", ..., name="admin")` is addressed as `admin:dashboard` (Starlette's `Mount.url_path_for` rule). The name index stores the fully qualified name, so namespaced routes neither collide with nor satisfy lookups for their bare name.

**REQ-ROUTE-11 — `url_for` call sites are recognized facts.**

Both spellings are recognized: `request.url_for(<name>, ...)` (the `Request` method) and `<obj>.url_path_for(<name>, ...)` where `<obj>` is an app or router — those objects expose `url_path_for`, not `url_for`. Each site is extracted with its first string argument and its literal keyword arguments. The capability specs consume these sites; the `url/*` validity checks live in the [F02 catalog](F02-diagnostics.md).

### 5.4 Capability surface

This spec owns the index; the user-facing features over it live in the capability specs — symbols ([F12](F12-symbols.md)), hover ([F10](F10-hover.md)), navigation ([F13](F13-navigation.md)), inlay hints ([F14](F14-inlay-hints.md)), completion ([F11](F11-completion.md)), and diagnostics ([F02](F02-diagnostics.md)).

## 6. Examples & Use Cases

You open the bookshop fresh. Workspace symbols for `books` lists three routes with full paths. You hover `get_book`: the card shows the chain through `main.py`'s include. The decorator says `/{book_id}` but an inlay hint adds `→ /api/books/{book_id}`. You ctrl-click `books.router` in `main.py` and land on the `APIRouter` line in `books.py`.

## 7. Edge Cases & Failure Modes

- Router included before it's defined in scan order → fine; linking runs on facts, not file order.
- Two methods on one handler (`@app.get` + `@app.head` stacked) → two route records, one handler location.
- `app.include_router(router)` with no prefix → chain link with empty prefix; resolution proceeds.
- Decorator on a lambda or non-`def` → fact dropped (no handler to anchor to).
- `APIRouter()` assigned to an attribute (`self.router`) → out of scope for v1; recorded as **OQ-ROUTE-1**.

## 8. Open Questions & Decisions

- **OQ-ROUTE-1** — Class-attribute routers (`self.router = APIRouter()`); revisit if real projects hit it.
- ~~OQ-ROUTE-2~~ — moved to [F12](F12-symbols.md) as OQ-SYM-1.

## Data Shapes & Code Map

Pass-1 facts and the enums that make "unresolved" a value, not an error:

```rust
// src/parsing/routes.rs — extraction output
pub struct RouteFact { pub object: String, pub methods: Vec<Method>, pub path: PathValue,
                       pub handler: Option<HandlerRef>, pub kwargs: RouteKwargs, pub name: Option<String> }
pub struct RouterDecl { pub name: String, pub prefix: PathValue, pub kwargs: RouterKwargs }
pub struct IncludeCall { pub object: String, pub target: DottedName, pub prefix: PathValue }
pub struct AppDecl { pub name: String, pub kind: AppKind }      // AppKind::{FastApi, Starlette}

pub enum PathValue { Literal(String), Constant(String, String), Unresolved }   // (name, value)
pub enum Method { Get, Post, Put, Delete, Patch, Options, Head, Trace, Websocket }

// src/linking.rs — chain resolution output (RouteRecord itself lives in E07)
pub struct ChainLink { pub kind: ChainKind, pub prefix: Option<String>, pub site: Location }
pub enum ChainKind { App, Include, RouterPrefix, Mount }
pub enum ResolvedPath { Resolved(String), Unresolved { known_suffix: String } }
```

Files: `parsing/routes.rs` (extraction), `linking.rs` (chain walk, trie build, name index). No error types — every failure mode is an `Unresolved` variant (constitution P3/P4).

## 9. Cross-References

- **Depends on:** [E01-architecture](../foundations/E01-architecture.md) — the passes; [E07-data-model](../foundations/E07-data-model.md) — `RouteRecord`, trie, REQ-IDX-01/02.
- **Related:** [F02](F02-diagnostics.md), [F04](F04-test-linking.md), [F06](F06-starlette-routing.md) — consumers of this index; [E15](../foundations/E15-app-config.md) — entrypoint hint for multi-app workspaces.

## 10. Changelog

- **2026-06-12** — Doc-verification fixes: path-converter syntax (`{id:int}`, multi-segment `{p:path}`); named-Mount name namespacing (`admin:dashboard`); `url_path_for` recognized alongside `url_for`; trailing-slash-prefix note. Touches [E07](../foundations/E07-data-model.md), [F02](F02-diagnostics.md).
- **2026-06-12** — Capability restructure: REQ-ROUTE-06…09 moved out to [F12](F12-symbols.md), [F10](F10-hover.md), [F14](F14-inlay-hints.md), [F13](F13-navigation.md); REQ-ROUTE-11 narrowed to fact extraction (features now in F11/F13). The gap in REQ numbering is intentional.
- **2026-06-12** — Added §5.7 `url_for` support: route-name indexing (REQ-ROUTE-10), completion and goto on `url_for` strings (REQ-ROUTE-11). Touches [E07](../foundations/E07-data-model.md) (`route_names`) and [F02](F02-diagnostics.md) (`url/*` codes).
- **2026-06-12** — Initial draft: extraction rules, chain resolution, symbols/hover/inlay/goto.
