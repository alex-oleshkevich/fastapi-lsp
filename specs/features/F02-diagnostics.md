# F02 — Diagnostics

> **Status:** Draft
>
> **Version:** 0.1   ·   **Last updated:** 2026-06-12
>
> **Purpose:** The framework-semantic checks the server publishes: path-parameter mismatches, duplicate and shadowed routes, and `Depends` misuse — each with a stable code.
>
> **Depends on:** [F01-route-index](F01-route-index.md), [constitution](../constitution.md)   ·   **Related:** [F03-dependency-graph](F03-dependency-graph.md), [F05-templates](F05-templates.md)

> Requirement tag: **DIAG**

---

## 1. Purpose & Scope

These are the bugs a type checker can't see because they live in string literals and cross-file wiring. Every diagnostic here is *positively provable* from the index — constitution P4 is the gate each check must pass.

This spec covers the diagnostic catalog, severities, ranges, and the publishing rules. Template diagnostics live in [F05](F05-templates.md); dependency-cycle detection lives in [F03](F03-dependency-graph.md) — both follow the catalog conventions defined here.

## 2. Non-Goals / Out of Scope

- Type errors of any kind — Pylance/ty's job (P5).
- Style opinions (route naming, REST conventions). We diagnose *wrong*, not *ugly*.
- Quick-fixes / code actions — deliberately deferred; see OQ-DIAG-2.

## 3. Detailed Specification

### 3.1 Publishing rules

**REQ-DIAG-01 — Diagnostics publish after pass 2, for every affected file.**

Cross-route checks (duplicates, shadows) can implicate a file the user isn't editing; after each relink the server re-publishes diagnostics for every file whose diagnostic set changed. Each diagnostic carries `source: "fastapi-lsp"` and its stable `code` from the catalog below.

**REQ-DIAG-02 — Unresolved means silent.**

A route with an unresolved path participates in no cross-route check, and an unresolvable `Depends` target raises nothing. Silence over speculation (P4).

**REQ-DIAG-09 — Every diagnostic carries its related code.**

Whenever a check involves a second location, the diagnostic's `relatedInformation` points there with a one-line label — the colliding route (`duplicate`, `shadowed`, `duplicate-name`), the shadowing route, each cycle member (`di/cycle`), the route a `url_for` resolved to (`param-mismatch` names the handler whose params didn't match), the `.env.example` line for a copyable env key. Unreachable-code checks (`route/shadowed`, `route/router-not-included`) additionally set `DiagnosticTag.Unnecessary`, so editors fade the dead code. Each diagnostic's `data` field carries the machine-readable payload its paired quick fix needs ([F08](F08-code-actions.md)), so `codeAction` requests don't recompute the analysis.

### 3.2 The catalog

| Code | Severity | Fires when |
|---|---|---|
| `route/param-missing-arg` | Error | A resolved path declares `{param}` but the handler signature has no parameter of that name. |
| `route/arg-missing-param` | Warning | A handler parameter is path-bound by elimination (no default, not a known dependency/body/query pattern) and matches no `{param}` in the path. |
| `route/duplicate` | Warning | Two resolved routes share method + identical path pattern (param *names* may differ — `/books/{id}` duplicates `/books/{book_id}`). |
| `route/shadowed` | Warning | A literal-segment route is registered after a param route that matches it (`/books/{id}` before `/books/featured` — the literal route is unreachable). |
| `route/router-not-included` | Warning | An `APIRouter` is defined but no `include_router`/`Mount` anywhere references it — its routes are unreachable. |
| `di/depends-called` | Error | `Depends(fn())` — the call's *return value*, produced once at import time, is passed where FastAPI expects the callable itself. The classic footgun. |
| `di/cycle` | Error | The dependency graph contains a cycle (detail in [F03 §3.4](F03-dependency-graph.md)). |
| `model/unknown-response-model` | Hint | `response_model=` names a symbol that is neither in `model_index` nor imported from outside the workspace. Hint-severity because the index is deliberately shallow (P5). |
| `model/unknown-body-model` | Hint | A handler body-param annotation names an unknown CamelCase symbol — same gates as the create-model action ([F08 §3.4](F08-code-actions.md)), which it pairs with. |
| `route/duplicate-name` | Warning | Two routes with *different handlers* share a route name, making `url_for` resolution ambiguous (first match wins silently). |
| `url/unknown-name` | Warning | A `url_for` string literal names no indexed route ([F01 §5.7](F01-route-index.md)). Starlette raises `NoMatchFound` at runtime on these. |
| `url/param-mismatch` | Warning | A `url_for` call's keyword arguments don't exactly cover the named route's path params — missing or extra names. |
| `tpl/missing-template` | Warning | A template reference matches no file under any template root ([F05](F05-templates.md)); suppressed when no roots exist (P4), includes the nearest-name suggestion at edit distance ≤ 2. |
| `env/undefined-key` | Information | An env lookup without a default names a key defined in no workspace env file (detail in [F09 §3.3](F09-env-settings.md)). |

### 3.3 The checks in detail

**REQ-DIAG-03 — `route/param-missing-arg` anchors to the path string.**

The squiggle covers the `{param}` segment inside the decorator's path literal — not the whole decorator — with message `path parameter '{book_id}' has no matching function parameter`. The check compares the *resolved* path's params against the handler signature, so prefix-contributed params (rare, but legal via router prefixes) count too.

**REQ-DIAG-04 — `route/arg-missing-param` is conservative by design.**

A parameter only fires this warning when every escape hatch is exhausted: it has no default, no `Depends`, isn't annotated with a known body/query/header marker (`Body`, `Query`, `Header`, `Cookie`, `Form`, `File`, `Request`, `Response`, a Pydantic model from `model_index`), and isn't named `self`/`cls`. FastAPI's real binding rules are type-driven; we only flag what is wrong under *every* interpretation (P4, P5).

**REQ-DIAG-05 — `route/duplicate` and `route/shadowed` report both ends.**

Each diagnostic carries `relatedInformation` pointing at the other route's decorator, so the user can jump between the two collision sites. Registration order for the shadow check follows chain order: includes in source order, then decorator source order within a router.

**REQ-DIAG-06 — The `url/*` checks suppress when the route set is incomplete.**

Both checks run on every `url_for` site — Python code *and* template files ([F05 REQ-TPL-06](F05-templates.md)). `url/unknown-name` fires only when every route source in the workspace is indexable. If any terminal mounted app exists (a third-party ASGI app per [F06](F06-starlette-routing.md)) it could register names we can't see, so absence is unprovable and the check stays silent (P4). `url/param-mismatch` has no such gate — once the name *does* match an indexed route, its path params are known exactly. The mismatch message lists both sides: `url_for('get_book') missing path param 'book_id'`. Kwargs with non-literal names (`**params`) suppress the check for that call.

**REQ-DIAG-07 — `route/duplicate-name` keys on handlers, not records.**

The name index maps one name to many route records legitimately: stacked method decorators, a router mounted twice, a multi-method `api_route` all produce several records for *one* handler and *one* honest name. The check therefore fires only when a name's records span **two or more distinct handlers within the same namespace** — names under different named Mounts are fully qualified (`admin:dashboard`, per [F01](F01-route-index.md) REQ-ROUTE-10) and never collide across namespaces. That same-namespace, distinct-handler case is where `url_for("name")` silently picks whichever registered first. The diagnostic anchors on each colliding route's `name=` kwarg (or the handler's `def` line when the name was defaulted from the function name), with `relatedInformation` pointing at the other holder(s). Like the other name-based checks, it needs no unresolved-path gate: names are prefix-independent (REQ-ROUTE-10).

**REQ-DIAG-08 — `route/router-not-included` fires on orphan routers, with escape hatches.**

The check anchors on the `APIRouter(...)` assignment when, after a complete scan, no include or mount edge (alias-aware, [E07 REQ-IDX-06](../foundations/E07-data-model.md)) resolves to it. Two suppressions keep it honest: the router's name appearing in an `__all__` literal (a deliberate library export), and any `Unresolved` include target existing in the workspace (the orphan might be *that* target — absence isn't proven). This diagnostic is the *explanation* for the router's routes showing `⟨unresolved⟩` paths; one squiggle at the cause beats one per route.

## 4. Examples & Use Cases

One worked example per code. The `~~~` marker shows where the squiggle lands; the comment is the message.

```python
# route/param-missing-arg — squiggle on the {param} segment
@router.get("/{book_id}")
#            ~~~~~~~~~  path parameter 'book_id' has no matching function parameter
def get_book(book: int): ...

# route/arg-missing-param — squiggle on the parameter
def get_book(book: int): ...
#            ~~~~  parameter 'book' matches no path parameter and has no binding marker

# route/duplicate — both decorators squiggled, relatedInformation links them
@router.get("/{book_id}")
def get_book(book_id: int): ...
@router.get("/{id}")
#            ~~~~~  duplicate of GET /api/books/{book_id} (param names differ, pattern is the same)
def get_book_again(id: int): ...

# route/shadowed — the unreachable literal route is squiggled
@router.get("/{book_id}")
def get_book(book_id: int): ...
@router.get("/featured")
#            ~~~~~~~~~  unreachable: GET /api/books/{book_id} above matches '/featured' first
def featured(): ...

# route/duplicate-name — name kwarg (or def line) squiggled on both holders
@router.get("/old", name="get_book")
#                        ~~~~~~~~~~  route name 'get_book' also used by get_book in books.py:12
def legacy(): ...

# route/router-not-included — the assignment is squiggled
admin_router = APIRouter(prefix="/admin")
# ~~~~~~~~~~~~  router is never included: no include_router or Mount references it

# di/depends-called — the call is squiggled
def list_books(db = Depends(get_db())):
#                           ~~~~~~~~  get_db is called here; its return value, created once
#                                     at import, is passed where FastAPI expects the callable
    ...

# di/cycle — the Depends argument continuing the cycle, per member
def get_a(b = Depends(get_b)): ...
#                     ~~~~~  dependency cycle: get_a → get_b → get_a

# model/unknown-response-model — the kwarg value (Hint severity)
@router.get("/", response_model=BookOut)
#                               ~~~~~~~  'BookOut' is not a known model in this workspace

# url/unknown-name — the name string
request.url_for("get_bok")
#                ~~~~~~~  no route is named 'get_bok' (did you mean 'get_book'?)

# url/param-mismatch — the call's argument list
request.url_for("get_book")
#               ~~~~~~~~~~  url_for('get_book') missing path param 'book_id'

# tpl/missing-template — the template string
templates.TemplateResponse(request, "book_lst.html", ctx)
#                                    ~~~~~~~~~~~~~~  template 'book_lst.html' not found under
#                                                    any template root (did you mean 'book_list.html'?)

# env/undefined-key — the key string (Information severity)
timeout = os.getenv("APP_TIMEOUT")
#                    ~~~~~~~~~~~  'APP_TIMEOUT' is not defined in workspace env files (.env, .env.example)

# model/unknown-body-model — the annotation (Hint severity)
@router.post("/")
def create_book(book: BookCreate): ...
#                     ~~~~~~~~~~  'BookCreate' is not a known model in this workspace
```

## 5. Edge Cases & Failure Modes

- Param mismatch in an *unresolved* route → the param check still runs (it only needs decorator path + signature); only cross-route checks are skipped.
- `*args`/`**kwargs` in a handler → suppresses `route/param-missing-arg` for that handler (the param may bind dynamically).
- Two mounts of the same router under different prefixes → not duplicates; patterns differ.
- Same path, different methods → never a duplicate.

## 6. Open Questions & Decisions

- **OQ-DIAG-1** — Severity of `route/shadowed`: Warning vs Information. Start Warning; downgrade if dogfooding shows intentional shadowing patterns.
- **Decision (resolves OQ-DIAG-2)** — Code actions are specified in [F08-code-actions](F08-code-actions.md); quick fixes for `di/depends-called`, `route/param-missing-arg`, and `route/arg-missing-param` ship with M2 alongside their checks.

## Data Shapes & Code Map

Every check is a pure function with one shape; the code enum keeps the catalog and the CLI's `--only`/`--ignore` honest:

```rust
// src/features/diagnostics.rs
pub enum DiagCode { RouteParamMissingArg, RouteArgMissingParam, RouteDuplicate, RouteShadowed,
                    RouteDuplicateName, RouteRouterNotIncluded, DiDependsCalled, DiCycle,
                    ModelUnknownResponseModel, ModelUnknownBodyModel, UrlUnknownName,
                    UrlParamMismatch, TplMissingTemplate, EnvUndefinedKey }
impl DiagCode { pub fn as_str(&self) -> &'static str; pub fn parse(s: &str) -> Option<Self> }

pub struct Finding { pub uri: Url, pub range: Range, pub code: DiagCode, pub severity: Severity,
                     pub message: String, pub related: Vec<(Location, String)>,
                     pub tags: Vec<DiagnosticTag>, pub data: Option<serde_json::Value> }

pub fn run_checks(state: &WorkspaceState, filter: &CodeFilter) -> Vec<Finding>;
```

Files: `features/diagnostics.rs` (dispatch, `Finding → lsp_types::Diagnostic`), one private module per check family (`checks/routes.rs`, `checks/di.rs`, `checks/url.rs`, …). `Finding` is also the `check` subcommand's unit of output ([F17](F17-cli.md) REQ-CLI-04).

## 7. Cross-References

- **Depends on:** [F01](F01-route-index.md) — resolved paths and the trie; [constitution](../constitution.md) — P4/P5 gates.
- **Related:** [F03](F03-dependency-graph.md) — `di/cycle`; [F05](F05-templates.md) — `tpl/missing-template`; [E17](../foundations/E17-testing.md) — broken fixtures asserting these positions.

## 8. Changelog

- **2026-06-12** — Added `route/router-not-included` (REQ-DIAG-08, with `__all__` and unresolved-include suppressions); §4 expanded into a worked example per code with squiggle positions and messages.
- **2026-06-12** — Doc-verification fixes: precise `di/depends-called` failure mode; `route/duplicate-name` scoped to same-namespace collisions (named Mounts qualify names).
- **2026-06-12** — Added `route/duplicate-name` (REQ-DIAG-07): fires across distinct handlers only, so stacked decorators and double mounts stay clean.
- **2026-06-12** — Added `env/undefined-key` row → [F09](F09-env-settings.md).
- **2026-06-12** — Added `url/unknown-name` and `url/param-mismatch` with the incomplete-route-set gate (REQ-DIAG-06); resolved OQ-DIAG-2 → [F08](F08-code-actions.md).
- **2026-06-12** — Initial draft: catalog of eight codes, publishing rules, conservatism gates.
