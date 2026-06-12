# E07 — Data Model

> **Status:** Draft
>
> **Version:** 0.1   ·   **Last updated:** 2026-06-12
>
> **Purpose:** The shape of `WorkspaceState` — the facts pass 1 stores and the indices pass 2 builds. Every feature reads from here.
>
> **Depends on:** [E01-architecture](E01-architecture.md)   ·   **Related:** [F01-route-index](../features/F01-route-index.md), [F03-dependency-graph](../features/F03-dependency-graph.md)

> Requirement tag: **IDX**

---

## 1. Purpose & Scope

`WorkspaceState` is the server's entire memory: raw per-file material, the facts extracted from it, and the linked indices computed from those facts. This spec defines each piece and who owns writing it.

## 2. Background & Rationale

The split mirrors the two passes. Pass 1 owns everything keyed by URI (replace-on-change); pass 2 owns everything keyed by workspace-level identity (rebuild-on-link). Keeping the ownership boundary crisp is what makes wholesale relinking safe: pass 2 can throw its indices away and rebuild from facts at any time.

## 3. Detailed Specification

### 3.1 The state struct

One struct, DashMaps throughout, shared as `Arc<WorkspaceState>` between the LSP handlers and the debounced linker.

```rust
// src/state.rs
pub struct WorkspaceState {
    // Pass-1 ownership: keyed by file, replaced atomically on change
    pub file_sources: DashMap<Url, String>,
    pub parse_trees:  DashMap<Url, Tree>,
    pub file_facts:   DashMap<Url, FileFacts>,

    // Pass-2 ownership: rebuilt wholesale on link
    pub route_index:    DashMap<RouteId, RouteRecord>,
    pub route_names:    DashMap<String, Vec<RouteId>>,  // url_for reverse lookup
    pub path_trie:      RwLock<PathTrie>,
    pub dep_graph:      RwLock<DepGraph>,
    pub template_index: DashMap<String, Url>,      // relative path → file
    pub model_index:    DashMap<String, ModelRecord>,  // is_settings flags BaseSettings models
    pub env_index:      DashMap<String, EnvEntry>,     // key → value + per-file locations (F09)
    pub middleware_classes: DashMap<String, MwInit>,   // class → __init__ kwargs (F16)
    pub test_refs:      DashMap<RouteId, Vec<ClientCallSite>>,

    pub config: RwLock<ResolvedConfig>,            // E15
}
```

### 3.2 Facts (pass-1 output)

`FileFacts` is the raw, unlinked harvest of one file. Names are stored as written; resolution happens later.

```rust
// src/state.rs
pub struct FileFacts {
    pub apps:          Vec<AppDecl>,        // FastAPI() / Starlette() assignments
    pub routers:       Vec<RouterDecl>,     // APIRouter(prefix=..., tags=...)
    pub includes:      Vec<IncludeCall>,    // include_router(target, prefix=...)
    pub routes:        Vec<RouteFact>,      // decorator or Route()/Mount() table entry
    pub dep_defs:      Vec<DepDef>,         // functions referenced by some Depends
    pub dep_refs:      Vec<DepRef>,         // each Depends(name) site
    pub templates:     Vec<TemplateRef>,    // TemplateResponse / get_template strings
    pub template_envs: Vec<TemplateEnvDecl>,// Jinja2Templates(directory=...)
    pub models:        Vec<ModelFact>,      // Pydantic BaseModel classes
    pub client_calls:  Vec<ClientCall>,     // client.get("/...") sites
    pub middlewares:   Vec<MiddlewareCall>, // add_middleware / Middleware(...) sites (F16)
}
```

A `RouteFact` carries the decorator path *as written* (`/{book_id}`), the methods, the handler's name and range, the kwargs that matter (`response_model`, `status_code`, `dependencies`), and the name of the object it was registered on (`app`, `router`). Prefix values that aren't string literals are looked up among module-level string constants; failing that they're stored as `PrefixValue::Unresolved`.

### 3.3 Linked indices (pass-2 output)

**REQ-IDX-01 — A route's identity survives relinking.**

`RouteId` is derived from stable coordinates — handler file + handler name + method — not from an insertion counter. Re-linking after an unrelated edit must yield the same IDs, or CodeLens and test-ref anchors would churn.

**REQ-IDX-02 — `RouteRecord` is the one truth about a route.**

Everything any feature says about a route comes from this record; no feature re-derives paths on its own.

```rust
// src/state.rs
pub struct RouteRecord {
    pub id: RouteId,
    pub name: String,                  // name= kwarg, else handler name (REQ-ROUTE-10)
    pub method: Method,
    pub resolved_path: ResolvedPath,   // Resolved("/api/books/{book_id}") | Unresolved
    pub decorator_path: String,        // "/{book_id}" as written
    pub chain: Vec<ChainLink>,         // app → include(prefix) → router(prefix)
    pub handler: Location,             // file + range of the function def
    pub path_params: Vec<PathParam>,   // name + converter: {id:int} → ("id", Int)
    pub response_model: Option<String>,
    pub dependencies: Vec<String>,     // direct Depends names (graph has the rest)
    pub middleware: Vec<String>,       // applied chain in execution order (F16 REQ-MW-04)
}
```

**REQ-IDX-03 — The path trie matches patterns and concrete paths alike.**

The trie stores resolved paths segment by segment; a `{param}` segment is a wildcard node. Two lookups exist: *pattern lookup* (exact segments, used for duplicate detection) and *concrete lookup* (`/api/books/1` walks literal and wildcard branches, used by [F04](../features/F04-test-linking.md)). A `{p:path}` converter node is a multi-segment wildcard: in concrete lookup it consumes one or more remaining segments; in pattern lookup it compares as its own node kind (so `/files/{p:path}` and `/files/{name}` are *different* patterns). Unresolved routes are not inserted (constitution P4).

**REQ-IDX-04 — The dependency graph is bidirectional.**

`DepGraph` keeps `uses: name → deps` and `used_by: name → users` adjacency, where a user is a handler or another dependency. Both directions are needed: goto walks forward, find-references walks backward ([F03](../features/F03-dependency-graph.md)).

### 3.4 Name binding

**REQ-IDX-06 — Name binding is import-alias-aware, everywhere.**

Wherever a spec says "import-aware" — include targets, `Depends` names, template envs, middleware classes — it means this rule: a name resolves through the referencing file's import statements **including aliases**, in all their forms:

```python
# every one of these binds b.router / db_dep / T to the same definitions
from app.routers import books as b          # b.router → routers/books.py's router
from app.deps import get_db as db_dep       # Depends(db_dep) → deps.py's get_db
import app.routers.books as books_mod       # books_mod.router → same router
from .templating import templates as tpl    # tpl.TemplateResponse(...) recognized
```

Relative imports resolve against the referencing file's package. Two aliases for one definition are one graph node — find-references returns both sites; `route/duplicate-name` and friends never see an alias as a second entity. A name binding that can't be resolved (star imports, `importlib`, re-export chains beyond one hop) is `Unresolved`, with the usual P4 consequences.

### 3.5 Invalidation

**REQ-IDX-05 — File change = replace facts, then relink.**

On change, the file's `FileFacts` entry is replaced (or removed on delete); the debounced pass 2 then rebuilds every pass-2 index from the surviving facts. No pass-2 structure is ever patched in place.

## 4. Examples & Use Cases

After indexing the bookshop, `route_index` holds three records. `get_book`'s record reads: method `GET`, resolved path `/api/books/{book_id}`, chain `[app, include(prefix="/api"), router(prefix="/books")]`, path params `[book_id]`, response model `Book`, dependencies `[get_db]`. Hover, symbols, diagnostics, and test linking all quote this one record.

## 5. Edge Cases & Failure Modes

- Two routers in different files share the variable name `router` → fine: `IncludeCall` targets are resolved through the importing file's imports, not by bare name globally.
- A handler is renamed → its `RouteId` changes (the name is part of identity); stale test-ref anchors die with the old ID at the next relink. This is correct: it *is* a different route now.
- `include_router` targets something never defined (typo, vendored code) → the include is recorded but links to nothing; routes on the orphan router stay `Unresolved`.

## 6. Open Questions & Decisions

- **OQ-IDX-1** — Should `model_index` store field types, or just names and ranges? Start with names + ranges; types are Pylance's job (P5) until a feature proves a need.

## 7. Cross-References

- **Depends on:** [E01-architecture](E01-architecture.md) — the two-pass ownership rule.
- **Related:** [F01](../features/F01-route-index.md) — fills routes; [F03](../features/F03-dependency-graph.md) — fills the graph; [F04](../features/F04-test-linking.md) — fills `test_refs`; [F05](../features/F05-templates.md) — fills `template_index`; [E15](E15-app-config.md) — fills `config`.

## 8. Changelog

- **2026-06-12** — Added §3.4 REQ-IDX-06: import-alias-aware name binding as the canonical definition of "import-aware" across the suite.
- **2026-06-12** — Added `middleware_classes` index and `middlewares` facts for [F16](../features/F16-middleware.md).
- **2026-06-12** — Doc-verification fixes: `PathParam` carries its converter; trie gains multi-segment `{p:path}` node semantics.
- **2026-06-12** — Added `env_index` and the `is_settings` flag on models for [F09](../features/F09-env-settings.md).
- **2026-06-12** — Added `route_names` index and `RouteRecord.name` for `url_for` support ([F01 §5.7](../features/F01-route-index.md)).
- **2026-06-12** — Initial draft: state struct, facts, linked indices, invalidation rules.
