# Constitution

> **Status:** Approved
>
> **Version:** 1.1   ·   **Last updated:** 2026-06-12
>
> **Purpose:** The governing rules for both the product and its specs — the principles fastapi-lsp must honor, and the conventions every spec in this suite follows.

---

## 1. Purpose & Scope

This document governs two things: the non-negotiable principles the language server is built on, and the authoring rules for every spec in this suite. When a spec and the constitution disagree, the constitution wins — fix the spec.

## 2. Product Principles

These are the rules the server must honor in every feature. Specs cite them as "per P3".

| # | Principle | What it means |
|---|---|---|
| P1 | Static analysis only | The server never imports, executes, or introspects user code. Everything comes from parsing source text with tree-sitter. |
| P2 | Editor-agnostic | Every feature ships as a standard LSP capability over stdio. No feature may depend on a single editor's proprietary API. |
| P3 | Never panic on partial code | Users edit mid-keystroke. Extractors return partial facts for broken syntax; the server must stay up and useful. |
| P4 | Only diagnose what is positively wrong | A diagnostic fires only when the code is provably incorrect from the indexed facts. Incomplete or unresolvable code gets silence, not guesses. |
| P5 | Complement the type checker, never duplicate it | Pylance/ty own types. This server owns framework semantics: routes, prefixes, dependencies, templates — things that live in string literals and decorator wiring. |
| P6 | Fast enough to forget it's there | Workspace scan completes in seconds on large projects; re-linking after an edit is debounced and cheap. A pure in-memory graph walk, never a re-parse of the world. |

## 3. Engineering Principles

- **Two-pass indexing.** Pass 1 extracts per-file facts; pass 2 links the workspace-level graphs (routers, dependencies, tests, templates). Per-file work happens on every keystroke; linking is debounced.
- **Features are pure functions.** Every LSP capability is a function of `&WorkspaceState` plus a position. No feature holds mutable state.
- **One parser.** tree-sitter-python is the only view of source code. No regex extraction, no second parser.
- **Boring, proven shape.** One binary, one parser, framework-agnostic LSP machinery (`tower-lsp-server` + tree-sitter + DashMap state) — the shape production Rust language servers converge on. Diverge only with a recorded decision.
- **Unresolved is a first-class state.** A route whose prefix can't be resolved statically is kept, marked `Unresolved`, and excluded from cross-route checks rather than guessed at (per P4).
- **Rejected: hybrid runtime introspection.** laravel-ls resolves framework magic by spawning PHP subprocesses against the user's app, cached per generation. We considered and rejected the equivalent (importing the user's app in a Python subprocess): it would need a working venv, trigger import side effects, add a security surface and cold-start latency — and FastAPI's wiring is statically visible enough that P1 costs us little. Recorded so the trade isn't re-litigated; revisit only if a feature is impossible without it.

## 4. Authoring Conventions

### 4.1 Document template

Every spec follows the suite template: the metadata header, then the numbered sections. Required sections are Purpose, Detailed Specification, Cross-References, and Changelog.

### 4.2 Naming & ID schemes

- **Files:** prefix + number + kebab slug. `E##` engineering foundations, `F##` features. The overview is `01-overview.md`; meta-docs are `index.md`, `constitution.md`, `glossary.md`. This suite has no UI, so the `D##` band is absent.
- **Reserved names:** foundation names follow the shared reserved-names registry — `E01` is always Architecture, `E07` always Data Model.
- **Requirement IDs:** each detailed spec declares a short uppercase tag (e.g. `ROUTE`); load-bearing rules are `REQ-ROUTE-01`, open questions `OQ-ROUTE-01`.
- **Diagnostic codes:** every diagnostic has a stable code in the form `area/short-name` (e.g. `route/duplicate`), defined in [F02-diagnostics](features/F02-diagnostics.md).

### 4.3 Crosslinking & the index

Specs link to each other inline and list every connection in their Cross-References section. The index is updated in the same edit as any spec change.

### 4.4 Status lifecycle & changelog

A spec moves `Draft → In Review → Approved`, and can end in one of two terminal states:

- **Deprecated** — was Approved, now superseded. Set the status and move the file to `deprecated/`.
- **Rejected** — considered and turned down. Set the status and move the file to `rejected/`.

Archived specs keep their name; the index lists them so the trail stays visible. Every change gets a dated changelog entry.

## 5. The Recurring Example Cast

Every spec draws its examples from the same small project: **the bookshop API**, a workspace the specs return to again and again.

- **`app/main.py`** — creates `app = FastAPI()` and wires `app.include_router(books.router, prefix="/api")`.
- **`app/routers/books.py`** — `router = APIRouter(prefix="/books", tags=["books"])` with handlers `list_books` (`GET /`), `get_book` (`GET /{book_id}`), and `create_book` (`POST /`). The fully resolved path of `get_book` is therefore `/api/books/{book_id}`.
- **`app/deps.py`** — `get_db()` yields a session; `get_current_user(db = Depends(get_db))` depends on it. The classic two-level dependency chain.
- **`app/models.py`** — `class Book(BaseModel)` with `id`, `title`, `author` fields, used as `response_model=Book`.
- **`app/pages.py`** — `templates = Jinja2Templates(directory="templates")` and a handler returning `templates.TemplateResponse("book_list.html", ...)`. The workspace has `templates/book_list.html` and `templates/base.html`.
- **`tests/test_books.py`** — `client.get("/api/books/1")` against a `TestClient(app)`.
- **`health/app.py`** — the raw-Starlette sibling: `Starlette(routes=[Route("/health", health), Mount("/static", app=StaticFiles(directory="static"))])`.

When a spec needs a mistake to illustrate a diagnostic, it breaks the bookshop: a `{book_id}` with no matching parameter, a second `GET /api/books/{book_id}`, a `Depends(get_db())` with the accidental call.

## 6. Visualization Style Guide

- **Mermaid** for flows, lifecycles, and graphs — init block, labeled arrows, semantic colors.
- **Tables** for index catalogs, diagnostic catalogs, and decision matrices.
- No ASCII screen mockups — the product has no screens; editor surfaces are described in prose.

## 7. Cross-References

- **Related:** [index](index.md), [glossary](glossary.md).

## 8. Changelog

- **2026-06-12** — v1.1: recorded the rejected hybrid runtime-introspection alternative (laravel-ls's model) under Engineering Principles.
- **2026-06-12** — Initial constitution: six product principles, the bookshop example cast, naming and diagnostic-code conventions.
