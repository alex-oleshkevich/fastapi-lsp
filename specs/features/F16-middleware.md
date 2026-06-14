# F16 — Middleware

> **Status:** Draft
>
> **Version:** 0.2   ·   **Last updated:** 2026-06-12
>
> **Purpose:** Recognizing middleware registration calls and indexing middleware constructor signatures, so their keyword arguments complete — the one place FastAPI hides a signature even from type checkers.
>
> **Depends on:** [E07-data-model](../foundations/E07-data-model.md)   ·   **Related:** [F11-completion](F11-completion.md)

> Requirement tag: **MW**

---

## 1. Purpose & Scope

`app.add_middleware(CORSMiddleware, ...)` forwards its kwargs through `**options` to the class constructor — so Pylance sees `Any` and completes nothing. The real signature exists; it's just hidden behind the indirection. This spec recovers it.

This spec covers:

- Recognition of middleware registration sites
- The two signature sources: workspace classes and the built-in stock table
- The kwarg completion surface (specified in [F11](F11-completion.md) REQ-CPL-06)

## 2. Non-Goals / Out of Scope

- Middleware ordering *diagnostics* — order matters at runtime, but no particular order is provably wrong statically (P4). The applied chain itself *is* modeled, in true execution order (REQ-MW-04), and rendered on the hover card.
- Completion of the middleware class name itself — a plain symbol, Pylance's job (P5).

## 3. Detailed Specification

### 3.1 Recognition

**REQ-MW-01 — Registration sites are recognized by shape.**

Pass 1 records `<obj>.add_middleware(<Class>, **kwargs)` where `<obj>` is an app or router-like name, and `Middleware(<Class>, **kwargs)` entries inside a `middleware=[...]` argument to `FastAPI(...)`/`Starlette(...)`, `Mount(...)`, `Route(...)`, or `Router(...)` — Starlette accepts middleware at every one of those levels. It also records `@<app>.middleware("http")` decorators — the most common way FastAPI users write custom middleware; that fact carries the decorated function's name, which is what the applied chain renders for it. Class registrations carry the class expression, the registration level, and the span of the argument list.

**REQ-MW-04 — Pass 2 computes each route's applied chain, in true execution order.**

Linking walks a route's chain and concatenates the middleware registered at each level — app, then mount/router, then route — storing the result on the `RouteRecord` ([E07](../foundations/E07-data-model.md)). The hover route card renders it ([F10](F10-hover.md) REQ-HOV-02).

Execution order is **not** source order, and getting this wrong renders the chain exactly backwards. `add_middleware` *prepends* — Starlette's implementation is `user_middleware.insert(0, …)` — so within one level those registrations apply in reverse source order: the last call is outermost and runs first. A `middleware=[...]` constructor list runs in list order, after that level's `add_middleware` registrations. Across levels the chain nests app → mount/router → route, outermost first. Decorator middleware (`@app.middleware("http")`) registers through the same prepend mechanism and follows the same rule.

### 3.2 Signature sources

**REQ-MW-02 — Workspace middleware classes contribute their `__init__` params.**

Any workspace class whose `__init__` takes `app` as its first non-`self` parameter is indexed as middleware-capable: its remaining `__init__` parameters (names, defaults, annotations as written) go into `middleware_classes` ([E07](../foundations/E07-data-model.md)). Resolution from the registration site to the class is import-aware, the same rule as include targets.

**REQ-MW-03 — Stock Starlette/FastAPI middleware comes from a built-in table.**

Stock classes aren't in the workspace, so their signatures ship in the server — a static table, versioned against the Starlette docs it was read from:

| Class | Kwargs |
|---|---|
| `CORSMiddleware` | `allow_origins`, `allow_methods`, `allow_headers`, `allow_credentials`, `allow_origin_regex`, `expose_headers`, `max_age` |
| `TrustedHostMiddleware` | `allowed_hosts`, `www_redirect` |
| `GZipMiddleware` | `minimum_size`, `compresslevel` |
| `SessionMiddleware` | `secret_key`, `session_cookie`, `max_age`, `path`, `same_site`, `https_only`, `domain` |
| `HTTPSRedirectMiddleware` | *(none)* |

Lookup order: workspace class first (it shadows the table — a vendored/subclassed copy is the one actually running), table second, no match → no completion.

### 3.3 The completion surface

Kwarg completion at a recognized registration site, after the class argument, is [F11](F11-completion.md) REQ-CPL-06: each kwarg completes as `allow_origins=` with the annotation and default as the detail.

## 4. Examples & Use Cases

You type `app.add_middleware(CORSMiddleware, ` — completion offers the seven CORS kwargs with their defaults, none of which any type checker can see. Your own `class TimingMiddleware: def __init__(self, app, header_name: str = "X-Time")` works identically via the workspace source.

## 5. Edge Cases & Failure Modes

- Class resolves to neither source (third-party middleware) → no completion, never a guess (P4).
- A workspace class *and* a table entry share a name → workspace wins (REQ-MW-03 order).
- Pure-ASGI function middleware (no class) → not recognized; nothing to complete.

## 6. Open Questions & Decisions

- **OQ-MW-1** — Warn on a kwarg not in the known signature (`mw/unknown-kwarg`)? Deferred: the table being stale would mint false positives; revisit once the table has a doc-sync check.

## Data Shapes & Code Map

```rust
// src/parsing/middleware.rs — facts
pub struct MiddlewareCall { pub source: MwSource, pub level: MwLevel, pub owner: String,
                            pub args_span: Range, pub present_kwargs: Vec<String> }
pub enum MwSource { Class(DottedName), DecoratorFn(String) }   // @app.middleware("http") → fn name
pub enum MwLevel { App, Router, Mount, Route }

// src/state.rs — signature index (workspace source)
pub struct MwInit { pub params: Vec<KwParam>, pub location: Location }
pub struct KwParam { pub name: String, pub annotation: Option<String>, pub default: Option<String> }

// src/parsing/middleware.rs — stock source (REQ-MW-03)
pub static STOCK_MIDDLEWARE: &[(&str, &[KwParamSpec])];                   // versioned against Starlette docs
```

Files: `parsing/middleware.rs` (recognition + stock table), `linking.rs` (per-route chain, REQ-MW-04).

## 7. Cross-References

- **Depends on:** [E07](../foundations/E07-data-model.md) — `middleware_classes`.
- **Related:** [F11](F11-completion.md) — the completion surface; [constitution](../constitution.md) — the P5 boundary this feature deliberately sits on the right side of.

## 8. Changelog

- **2026-06-12** — v0.2 review pass: REQ-MW-04 states Starlette's real ordering (`add_middleware` prepends — reverse source order within a level; constructor lists in list order); REQ-MW-01 recognizes `@app.middleware("http")` decorators; Non-Goals narrowed to ordering *diagnostics* (the chain itself is modeled); `MwSource` in data shapes.
- **2026-06-12** — Added mount/route/router-level registration and REQ-MW-04 (per-route applied chain, rendered by the hover card).
- **2026-06-12** — Initial draft: registration recognition, dual signature sources, stock table.
