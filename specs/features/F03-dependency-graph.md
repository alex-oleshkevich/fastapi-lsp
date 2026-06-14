# F03 — Dependency Graph

> **Status:** Draft
>
> **Version:** 0.2   ·   **Last updated:** 2026-06-12
>
> **Purpose:** Making `Depends()` navigable: goto and find-references through the dependency chain, usage counts on hover, and cycle detection.
>
> **Depends on:** [F01-route-index](F01-route-index.md), [E07-data-model](../foundations/E07-data-model.md)   ·   **Related:** [F02-diagnostics](F02-diagnostics.md)

> Requirement tag: **DI**

---

## 1. Purpose & Scope

FastAPI's dependency system is a call graph the framework assembles at runtime — which means no static tool shows it to you. This spec makes that graph a first-class, navigable structure.

This spec covers:

- Pass-1 extraction of `Depends` references and the functions they name
- The bidirectional graph built in pass 2
- `dependency_overrides` write sites and the `di/override-unused` diagnostic
- Goto definition, find references, hover, and the `di/cycle` diagnostic

## 2. Non-Goals / Out of Scope

- Resolving what a dependency *returns* (type-level — P5).
- The `di/depends-called` check — cataloged in [F02](F02-diagnostics.md).

## 3. Detailed Specification

### 3.1 Extraction

**REQ-DI-01 — Every `Depends(name)` site becomes an edge candidate.**

Pass 1 records each `Depends(<expr>)` where `<expr>` is a name or dotted name, in any of its homes: handler parameter defaults, `Annotated[T, Depends(name)]` metadata, other dependencies' parameters, `APIRouter(dependencies=[...])`, route-decorator `dependencies=[...]`, `app = FastAPI(dependencies=[...])`, and `include_router(..., dependencies=[...])` — the "Bigger Applications" tutorial pattern. The user of the edge is the enclosing function (or router/app for the list forms; for `include_router`, the *included* router). The include form rides on [F01](F01-route-index.md)'s `IncludeCall` capture.

A bare `Depends()` is also an edge: FastAPI takes the dependency from the parameter's annotation, and that annotation is statically visible — `Annotated[CommonParams, Depends()]` (or `c: CommonParams = Depends()`) resolves to the class `CommonParams` exactly as if it were written inside the call.

### 3.2 The graph

**REQ-DI-02 — Edges resolve by import-aware name binding, bidirectionally.**

Pass 2 binds each recorded name to a function definition — local file first, then through the file's imports — and inserts the edge in both adjacency maps (`uses`, `used_by`, per [E07 REQ-IDX-04](../foundations/E07-data-model.md)). Unbindable names produce no edge and no diagnostic (P4).

In the bookshop: `get_current_user` *uses* `get_db`; `get_db` is *used by* `get_current_user` and (directly) by `list_books`.

### 3.3 Capability surface

This spec owns the graph; its user-facing features live in the capability specs — goto and find references in [F13](F13-navigation.md), the dependency hover card in [F10](F10-hover.md) REQ-HOV-04.

### 3.4 Cycle detection

**REQ-DI-04 — Cycles are errors, reported once per cycle.**

After each relink, a DFS over `uses` finds strongly-connected components of size > 1 (or self-loops). Each cycle raises one `di/cycle` diagnostic per member, anchored on the member's `Depends(...)` argument that continues the cycle, with `relatedInformation` walking the loop: `dependency cycle: get_a → get_b → get_a`. A cyclic chain can never resolve, and FastAPI documents no guard for it — catching it at edit time is strictly earlier than any runtime failure.

### 3.5 Test overrides

**REQ-DI-05 — `dependency_overrides` sites join the graph's references.**

Tests swap dependencies through `app.dependency_overrides` — and a rename that misses `conftest.py` leaves an override silently pointing at nothing. Pass 1 records both write shapes: the subscript assignment `app.dependency_overrides[get_db] = fake_db` and the call `app.dependency_overrides.update({get_db: fake_db})`. The overridden name binds like any other edge (REQ-DI-02), and find-references on a dependency lists its override sites alongside its `Depends(...)` sites — so the "every consumer" promise in §4 covers the test suite too.

When the overridden callable is not a node in the dependency graph, an Information diagnostic `di/override-unused` fires on the key. The override can never take effect — which is exactly what a rename leaves behind. Its catalog row lives in [F02](F02-diagnostics.md). The dependency hover card ([F10](F10-hover.md) REQ-HOV-04) gains an "overridden in tests/conftest.py" line when override sites exist.

## 4. Examples & Use Cases

You're refactoring `get_db` to return an async session and need every consumer. Find-references on `get_db` lists the `Depends(get_db)` in `list_books`'s signature and the one inside `get_current_user` — including injection sites in files Pylance's reference search reports only as plain name usages mixed in with imports and tests. It also lists the `app.dependency_overrides[get_db]` line in `tests/conftest.py`, which a rename would otherwise orphan. Hover on `get_current_user` shows it sits one level above `get_db` and is used by no route yet.

## 5. Edge Cases & Failure Modes

- A dependency defined in an installed package (`Depends(oauth2_scheme)` from a library) → no edge, no complaint; only workspace-defined functions join the graph.
- Class dependencies (`Depends(CommonParams)`) → the name binds to the class definition; goto and references work the same.
- One function injected under two names (aliased import) → both bind to the same definition; references find both sites.
- `use_cache=False` and other `Depends` kwargs → ignored; they don't change the graph shape.

## 6. Open Questions & Decisions

- **OQ-DI-1** — Should hover show the *transitive* closure ("resolves 3 dependencies deep") or direct edges only? Start direct-only; transitive on a real need.

## Data Shapes & Code Map

```rust
// src/parsing/deps.rs — facts
pub struct DepDef { pub name: String, pub location: Location }           // def the graph can target
pub struct DepRef { pub name: DottedName, pub user: DepUser, pub home: DepHome, pub site: Range }
pub enum DepUser  { Handler(HandlerRef), Dependency(String), Router(String), App(String) }
pub enum DepHome  { ParamDefault, Annotated, AnnotatedBare,              // Depends() ← annotation
                    DecoratorList, RouterList, AppList, IncludeList }    // include_router(dependencies=[...])
pub struct OverrideSite { pub overridden: DottedName, pub site: Range }  // dependency_overrides writes

// src/linking.rs — the graph (bidirectional, E07 REQ-IDX-04)
pub struct DepGraph { pub uses: HashMap<NodeId, Vec<NodeId>>, pub used_by: HashMap<NodeId, Vec<NodeId>>,
                      pub sites: HashMap<(NodeId, NodeId), Vec<Range>> }
pub fn cycles(&self) -> Vec<Vec<NodeId>>;                                // SCCs of size > 1 + self-loops
```

Files: `parsing/deps.rs` (facts), `linking.rs` (binding + graph), `checks/di.rs` (cycle check). Unbindable names create no node — there is no error variant to handle downstream. `NodeId` is defined in [E07](../foundations/E07-data-model.md) as the definition's location (`Uri` + range of the `def`/`class`); names are display-only.

## 7. Cross-References

- **Depends on:** [F01](F01-route-index.md) — handlers as graph users; `IncludeCall`'s `dependencies=[...]` capture; [E07](../foundations/E07-data-model.md) — REQ-IDX-04's bidirectional maps and `NodeId`.
- **Related:** [F02](F02-diagnostics.md) — catalog conventions for `di/cycle` and `di/override-unused`; [F10](F10-hover.md) — the hover card's override line.

## 8. Changelog

- **2026-06-12** — Review pass: `dependency_overrides` writes are extracted facts with override sites in find-references and the new `di/override-unused` check (REQ-DI-05); `include_router(dependencies=[...])` recognized as a `Depends` home; `NodeId` identity clarified per E07.
- **2026-06-12** — Doc-verification fixes: bare `Depends()` (annotation-derived) is statically resolvable and now an edge source — removed from Non-Goals; softened the unverified runtime-failure claim in the cycle section.
- **2026-06-12** — Capability restructure: REQ-DI-03 moved out to [F13](F13-navigation.md) and [F10](F10-hover.md).
- **2026-06-12** — Initial draft: edge extraction homes, bidirectional graph, navigation, cycles.
