# E07 — Data Model

> **Status:** Draft
>
> **Version:** 0.2   ·   **Last updated:** 2026-06-12
>
> **Purpose:** The shape of `WorkspaceState` — the facts pass 1 stores and the indices pass 2 builds. Every feature reads from here.
>
> **Depends on:** [E01-architecture](E01-architecture.md)   ·   **Related:** [F01-route-index](../features/F01-route-index.md), [F03-dependency-graph](../features/F03-dependency-graph.md)

> Requirement tag: **IDX**

---

## 1. Purpose & Scope

`WorkspaceState` is the server's entire memory: raw per-file material, the facts extracted from it, and the linked indices computed from those facts. This spec defines each piece and who owns writing it.

## 2. Background & Rationale

The split mirrors the two passes. Pass 1 owns everything keyed by URI (replace-on-change); pass 2 owns everything keyed by workspace-level identity (rebuild-on-link). Keeping the ownership boundary crisp is what makes wholesale relinking safe: pass 2 builds a fresh snapshot from the facts at any time and swaps it in atomically.

## 3. Detailed Specification

### 3.1 The state struct

One struct, shared as `Arc<WorkspaceState>` between the LSP handlers and the debounced linker. The two passes get two different concurrency tools: pass-1 state lives in DashMaps (fine-grained, per-URI writes), while everything pass 2 produces is one immutable snapshot behind an `ArcSwap`.

```rust
// src/state.rs
pub struct WorkspaceState {
    // Pass-1 ownership: keyed by URI, replaced atomically on change
    pub file_facts:     DashMap<Uri, FileFacts>,
    pub file_sources:   DashMap<Uri, String>,  // open documents only (§3.6)
    pub parse_trees:    DashMap<Uri, Tree>,    // open documents only (§3.6)
    pub template_facts: DashMap<Uri, Vec<TemplateUrlForSite>>, // url_for sites in templates (F05)
    pub open_docs:      DashSet<Uri>,          // maintained by didOpen/didClose

    // Pass-2 ownership: one immutable snapshot, swapped atomically on link
    pub linked: arc_swap::ArcSwap<Linked>,

    pub config: RwLock<ResolvedConfig>,        // E15
}
```

`Linked` is everything pass 2 knows, frozen. Because the struct is immutable once published, plain `HashMap`s suffice inside — no locks, no concurrent maps:

```rust
// src/state.rs
pub struct Linked {
    pub generation:     u64,                          // the pass-1 generation this was linked from (E01 REQ-ARCH-04)
    pub route_index:    HashMap<RouteId, Vec<RouteRecord>>, // one record per mount instance (REQ-IDX-01)
    pub route_names:    HashMap<String, Vec<RouteId>>,      // url_for reverse lookup
    pub path_trie:      PathTrie,
    pub dep_graph:      DepGraph,
    pub template_index: HashMap<String, Uri>,         // relative path → file
    pub model_index:    HashMap<String, Vec<ModelRecord>>,  // is_settings flags BaseSettings models
    pub env_index:      HashMap<String, EnvEntry>,    // key → value + per-file locations (F09)
    pub middleware_classes: HashMap<String, Vec<MwInit>>,   // class → __init__ kwargs (F16)
    pub test_refs:      HashMap<RouteId, Vec<ClientCallSite>>,
}
```

Pass 2 builds a fresh `Linked` off to the side, then publishes it with a single `linked.store(Arc::new(new))`. A feature loads the snapshot **once** at the top of its request (`state.linked.load()`) and answers entirely from it — every answer is internally consistent, even if a relink lands mid-request.

> **Warning:** No DashMap guard may be held across an `.await`, or across access to another map. Take the entry, clone what you need, drop the guard. This is the deadlock rule for the whole codebase.

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

**REQ-IDX-01 — A route's identity survives relinking; a route may have many mount instances.**

`RouteId` is derived from stable coordinates — handler file + handler name + method — not from an insertion counter. Re-linking after an unrelated edit must yield the same IDs, or CodeLens and test-ref anchors would churn.

One ID can map to several records. A router included twice (say, mounted under both `/api` and `/v2`) produces one `RouteId` with one `RouteRecord` per mount instance — that's why `route_index` is `HashMap<RouteId, Vec<RouteRecord>>`.

Features split along that line. Features that anchor on the handler — CodeLens, hover, test-ref anchors, find-references — key on the `RouteId` and aggregate the `Vec`. Features about the URL space — the path trie, `route/duplicate` and `route/shadowed`, path completion, `url_for` resolution — iterate every mount instance as its own route.

**REQ-IDX-02 — `RouteRecord` is the one truth about a route.**

Everything any feature says about a route comes from this record; no feature re-derives paths on its own.

```rust
// src/state.rs
pub struct RouteRecord {
    pub id: RouteId,
    pub ordinal: u32,                  // registration order, assigned by pass 2 (see below)
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

The `ordinal` is the route's registration order: pass 2 assigns it while walking includes and decorators in source order, mirroring the order Starlette would register the routes at runtime. `route/shadowed` ([F02](../features/F02-diagnostics.md)) consumes it — a route can only be shadowed by one registered *earlier*.

Same-name classes need the same treatment as multiply-mounted routers. `model_index` and `middleware_classes` map a name to a `Vec` of records, and each record carries its defining module — two `Book` classes in different modules are two entries under one key. [F08](../features/F08-code-actions.md)'s "exactly one definition" gate is then simply `len() == 1`.

**REQ-IDX-03 — The path trie matches patterns and concrete paths alike.**

The trie stores resolved paths segment by segment; a `{param}` segment is a wildcard node. Two lookups exist: *pattern lookup* (exact segments, used for duplicate detection) and *concrete lookup* (`/api/books/1` walks literal and wildcard branches, used by [F04](../features/F04-test-linking.md)). A `{p:path}` converter node is a multi-segment wildcard: in concrete lookup it consumes one or more remaining segments; in pattern lookup it compares as its own node kind (so `/files/{p:path}` and `/files/{name}` are *different* patterns). Unresolved routes are not inserted (constitution P4).

**REQ-IDX-04 — The dependency graph is bidirectional, keyed by definition.**

A graph node is a *definition*, not a name. `NodeId` is the definition's location — the `Uri` plus the range of the `def`/`class` — with the display name stored separately. Two functions that happen to share a name are two nodes; two aliases for one function are one node.

`DepGraph` keeps `uses: definition → definitions` and `used_by: definition → definitions` adjacency, where a user is a handler or another dependency. Both directions are needed: goto walks forward, find-references walks backward ([F03](../features/F03-dependency-graph.md)).

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

Turning a module path like `app.routers.books` into a file needs a set of **source roots**, and the server infers them: the workspace root, plus `src/` when it exists, plus any roots declared in `pyproject.toml`, plus the explicit `source_roots` config ([E15](E15-app-config.md)). That covers flat layouts, `src/` layouts, and monorepos. When no root resolves a module path, the binding degrades to `Unresolved` — the server never guesses.

### 3.5 Invalidation

**REQ-IDX-05 — File change = replace facts, then link a fresh snapshot.**

On change, the file's `FileFacts` entry is replaced (or removed on delete) and the workspace generation is bumped. The debounced pass 2 then builds a complete new `Linked` from the surviving facts and swaps it in with one `ArcSwap::store`.

No pass-2 structure is ever patched in place, and nothing is ever cleared before its replacement exists — readers see either the old snapshot or the new one, never an empty or half-built window in between. If the generation moved while pass 2 was linking, the finished snapshot is stale: it is discarded unpublished and pass 2 reschedules ([E01 REQ-ARCH-04](E01-architecture.md)).

### 3.6 Retention

What the server keeps in memory depends on whether the document is open.

**REQ-IDX-07 — Facts for everything; sources and trees for open documents only.**

`file_facts` (and `template_facts`) are kept for every indexed file — they are what pass 2 links, so they must cover the whole workspace. `file_sources` and `parse_trees` are kept only for documents in `open_docs`; for closed files they are re-derived from disk on demand.

On `didClose`, the URI leaves `open_docs`, its source and tree are dropped, and its facts are re-extracted from disk — the on-disk content may differ from the abandoned buffer. The facts themselves stay. On `didOpen`, the URI joins `open_docs` and the editor's buffer becomes the truth; watcher events for open documents are ignored ([E01 REQ-ARCH-12](E01-architecture.md)).

## 4. Examples & Use Cases

After indexing the bookshop, `route_index` holds three entries, each with a single record — no bookshop router is mounted twice. `get_book`'s record reads: method `GET`, resolved path `/api/books/{book_id}`, chain `[app, include(prefix="/api"), router(prefix="/books")]`, path params `[book_id]`, response model `Book`, dependencies `[get_db]`. Hover, symbols, diagnostics, and test linking all quote this one record.

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

- **2026-06-12** — v0.2: Review-fix batch — pass-2 indices moved into an immutable `Linked` snapshot behind `ArcSwap` (plain `HashMap`s inside, per-index `RwLock`s removed, one-load-per-request rule, no-guard-across-`.await` invariant); `Url` → `Uri` throughout; `route_index` becomes `HashMap<RouteId, Vec<RouteRecord>>` with one record per mount instance; `model_index`/`middleware_classes` keyed name → `Vec` of module-carrying records; `NodeId` = definition location with display name separate (REQ-IDX-04); `RouteRecord.ordinal` for `route/shadowed`; new `template_facts` and `open_docs` pass-1 state; §3.6 retention rules and `didClose` semantics (REQ-IDX-07); REQ-IDX-05 rewritten for snapshot swap + generation counter; source-root inference rule in §3.4.
- **2026-06-12** — Added §3.4 REQ-IDX-06: import-alias-aware name binding as the canonical definition of "import-aware" across the suite.
- **2026-06-12** — Added `middleware_classes` index and `middlewares` facts for [F16](../features/F16-middleware.md).
- **2026-06-12** — Doc-verification fixes: `PathParam` carries its converter; trie gains multi-segment `{p:path}` node semantics.
- **2026-06-12** — Added `env_index` and the `is_settings` flag on models for [F09](../features/F09-env-settings.md).
- **2026-06-12** — Added `route_names` index and `RouteRecord.name` for `url_for` support ([F01 §5.7](../features/F01-route-index.md)).
- **2026-06-12** — Initial draft: state struct, facts, linked indices, invalidation rules.
