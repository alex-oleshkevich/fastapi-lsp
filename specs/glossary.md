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
| **Dependency** | A callable referenced by `Depends(...)` — in the bookshop, `get_db` and `get_current_user`. Dependencies can depend on other dependencies, forming the dependency graph. |
| **Dependency graph** | The workspace-level directed graph of dependencies and their users (handlers and other dependencies), built in pass 2. Owned by [F03](features/F03-dependency-graph.md). |
| **Env index** | The map of env keys to their values and locations across `.env` / `.env.example`. Owned by [F09](features/F09-env-settings.md). |
| **Fact** | A single raw extraction from pass 1 — a route decorator, an `APIRouter` declaration, an `include_router` call, a `Depends` reference, a template usage. Facts are per-file and unlinked. |
| **Handler** | The function a route decorator wraps (or a `Route(...)`'s endpoint) — `get_book` in the bookshop. Also called a path operation in FastAPI docs. |
| **Import-aware binding** | Resolving a name through the referencing file's imports, including aliases (`import x as y`, `from m import n as a`) and relative imports. Canonical rule: [E07 REQ-IDX-06](foundations/E07-data-model.md). |
| **Indicator** | A cheap substring check (e.g. `"from fastapi"`) deciding whether a file is worth parsing during a workspace scan. |
| **Linking (pass 2)** | The debounced workspace-level pass that connects facts into graphs: resolving router chains into final paths, binding `Depends` names to definitions, matching test calls to routes. |
| **Path trie** | The index mapping path patterns to routes, segment by segment, with parameter segments as wildcards. Powers duplicate detection and test-call matching. |
| **Resolved path** | A route's final URL after walking its router chain: app prefix + each `include_router` prefix + the router's own prefix + the decorator path. `get_book` resolves to `/api/books/{book_id}`. |
| **Route** | One method + resolved path + handler triple. `GET /api/books/{book_id}` → `get_book` is one route; a decorator with two methods yields two routes. |
| **Route name** | A route's reverse-lookup identifier, used by `url_for`: the `name=` kwarg when given, else the handler's function name — Starlette's own rule. Prefix-independent. |
| **Settings model** | A class inheriting `BaseSettings` (pydantic-settings); each field binds to an env key by pydantic's naming rules. Flagged `is_settings` in the model index. |
| **Router chain** | The sequence of routers a route passes through from the app to its decorator — for `get_book`: `app` → (`prefix="/api"`) → `books.router` (`prefix="/books"`). |
| **Template root** | A directory whose files are addressable as templates, resolved from the `templates` config key, `Jinja2Templates(directory=...)` detection, or the `templates/` fallback — see [E15](foundations/E15-app-config.md). |
| **Unresolved** | The state of a route or prefix that cannot be computed statically (e.g. `prefix=get_prefix()`). Unresolved routes stay indexed for navigation but are excluded from cross-route diagnostics, per constitution P4. |
| **WorkspaceState** | The server's whole in-memory model: per-file sources, parse trees, facts, and the linked indices. Defined in [E07-data-model](foundations/E07-data-model.md). |

## Changelog

- **2026-06-12** — Initial glossary.
