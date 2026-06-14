# Glossary

> **Status:** Living (continuously maintained)
>
> **Last updated:** 2026-06-12
>
> **Purpose:** The canonical definition of every domain term the suite uses. Defined once here, linked everywhere else.

---

Terms are alphabetical. When a spec introduces a new term, it gets a row here in the same edit.

| Term | Definition |
|---|---|
| **Applied chain** | The ordered list of middleware a request actually passes through. `add_middleware` prepends, so those calls apply in reverse source order — the last call is outermost — followed by the `middleware=[...]` constructor list in list order. Owned by [F16](features/F16-middleware.md). |
| **Capability spec** | A feature spec that owns one LSP capability's user-facing behavior across all domains — F02, F08, F10–F15. Contrast with a domain spec. |
| **Client call** | A `TestClient`/httpx verb call in a test — `client.get("/api/books/1")` — matched against the route index by [F04](features/F04-test-linking.md). |
| **Dependency** | A callable referenced by `Depends(...)` — in the bookshop, `get_db` and `get_current_user`. Dependencies can depend on other dependencies, forming the dependency graph. |
| **Dependency graph** | The workspace-level directed graph of dependencies and their users (handlers and other dependencies), built in pass 2. Owned by [F03](features/F03-dependency-graph.md). |
| **Domain spec** | A feature spec that owns one domain's indexing semantics — what's extracted and linked: F01, F03–F06, F09, F16. Contrast with a capability spec. |
| **Env index** | The map of env keys to their values and locations across `.env` / `.env.example`. Owned by [F09](features/F09-env-settings.md). |
| **Fact** | A single raw extraction from pass 1 — a route decorator, an `APIRouter` declaration, an `include_router` call, a `Depends` reference, a template usage. Facts are per-file and unlinked. |
| **Handler** | The function a route decorator wraps (or a `Route(...)`'s endpoint) — `get_book` in the bookshop. Also called a path operation in FastAPI docs. |
| **Import-aware binding** | Resolving a name through the referencing file's imports, including aliases (`import x as y`, `from m import n as a`) and relative imports. Canonical rule: [E07 REQ-IDX-06](foundations/E07-data-model.md). |
| **Indicator** | A cheap substring check (e.g. `"from fastapi"`) deciding whether a file is worth parsing during a workspace scan. |
| **Linking (pass 2)** | The debounced workspace-level pass that connects facts into graphs: resolving router chains into final paths, binding `Depends` names to definitions, matching test calls to routes. |
| **Path trie** | The index mapping path patterns to routes, segment by segment, with parameter segments as wildcards. Powers duplicate detection and test-call matching. |
| **Resolved path** | A route's final URL, built by walking the router chain from the app object: each `include_router` or mount prefix, then the router's own prefix, then the decorator path. `get_book` resolves to `/api/books/{book_id}`. `root_path` is proxy metadata — it never affects matching and plays no part here. |
| **Route** | One method + resolved path + handler triple. `GET /api/books/{book_id}` → `get_book` is one route; a decorator with two methods yields two routes. |
| **Route name** | A route's reverse-lookup identifier, used by `url_for`: the `name=` kwarg when given, else the handler's function name — Starlette's own rule. Prefix-independent. |
| **Router chain** | The sequence of routers a route passes through from the app to its decorator — for `get_book`: `app` → (`prefix="/api"`) → `books.router` (`prefix="/books"`). |
| **Settings model** | A class inheriting `BaseSettings` (pydantic-settings); each field binds to an env key by pydantic's naming rules. Flagged `is_settings` in the model index. |
| **Template root** | A directory whose files are addressable as templates, resolved from the `templates` config key, `Jinja2Templates(directory=...)` detection, or the `templates/` fallback — see [E15](foundations/E15-app-config.md). |
| **Terminal mount** | A `Mount` wrapping a non-router app — `StaticFiles(directory="static")`, a sub-application — where path resolution stops. Named terminal mounts still join `route_names` so `url_for("static", path=...)` resolves. Owned by [F06](features/F06-starlette-routing.md). |
| **Unresolved** | The state of a route or prefix that cannot be computed statically (e.g. `prefix=get_prefix()`). Unresolved routes stay indexed for navigation but are excluded from cross-route diagnostics, per constitution P4. |
| **WorkspaceState** | The server's whole in-memory model: per-file sources, parse trees, facts, and the linked indices. Defined in [E07-data-model](foundations/E07-data-model.md). |

## Changelog

- **2026-06-12** — Review-pass update, with an honest backfill: earlier batches had added **route name**, **env index**, **settings model**, and **import-aware binding** without changelog entries — recorded now. Today's pass adds **domain spec**, **capability spec**, **applied chain**, **client call**, and **terminal mount**; rewords **Resolved path** (the chain starts at the app object, prefixes come from `include_router`/mounts, and `root_path` is proxy metadata that never affects matching); and re-sorts the table alphabetically.
- **2026-06-12** — Initial glossary.
