# F04 — Test Linking

> **Status:** Draft
>
> **Version:** 0.1   ·   **Last updated:** 2026-06-12
>
> **Purpose:** Connecting test code to the routes it exercises: goto from `client.get("/api/books/1")` to the handler, CodeLens counting test references on handlers, and path completion inside client calls.
>
> **Depends on:** [F01-route-index](F01-route-index.md), [E07-data-model](../foundations/E07-data-model.md)   ·   **Related:** [F02-diagnostics](F02-diagnostics.md)

> Requirement tag: **TLINK**

---

## 1. Purpose & Scope

A test calling `client.get("/api/books/1")` and the handler serving it are joined only by a string the framework will parse at runtime. This spec joins them at edit time, in both directions.

This spec covers:

- Pass-1 extraction of HTTP-client call sites
- Concrete-path matching through the trie
- Goto definition (test → handler), find references and CodeLens (handler → tests), and path completion in client-call strings

## 2. Non-Goals / Out of Scope

- Running tests or endpoints (P1).
- Asserting anything about request/response bodies.
- Non-literal paths (`client.get(url)` where `url` is computed) — skipped silently, per P4.

## 3. Detailed Specification

### 3.1 Extraction

**REQ-TLINK-01 — Client calls are recognized by shape, not by type.**

Pass 1 records `<obj>.<verb>(<string-literal>, ...)` where `<verb>` is an HTTP method name (`get`, `post`, `put`, `delete`, `patch`, `options`, `head`) and `<obj>` is a name bound — in file or fixture scope — to a `TestClient(...)`, `httpx.Client(...)`, or `httpx.AsyncClient(...)` construction, or a pytest fixture parameter whose name is `client`/`async_client`. The shape rule keeps us honest without type inference; the fixture-name heuristic is the one pragmatic concession, and it's confined to test files (`test_*.py` / `*_test.py` / `tests/` subtrees).

### 3.2 Matching

**REQ-TLINK-02 — Concrete paths match through the trie, exactly.**

The literal path (query string stripped) walks the path trie's concrete lookup: literal segments must match exactly; a `{param}` node matches any single segment. `client.get("/api/books/1")` matches `GET /api/books/{book_id}`. The verb must match the route's method. Zero matches → no link; two or more matches (ambiguous wildcards) → link to all, letting the editor show a picker.

Matches are stored in `test_refs` (route → call sites) at link time, so both directions below are pure lookups.

### 3.3 Capability surface

This spec owns the matching; the features over it live in the capability specs — goto and references in [F13](F13-navigation.md), the test-reference lens in [F15](F15-code-lens.md), path completion in [F11](F11-completion.md) REQ-CPL-02.

## 4. Examples & Use Cases

In `tests/test_books.py` you type `client.get("` — completion lists the bookshop's two GET paths. You pick `/api/books/{book_id}`, fill in `1`, and later ctrl-click the string to land in `get_book`. Back in `books.py`, `get_book` now wears a `▶ 1 test reference` lens; clicking it jumps to the call.

## 5. Edge Cases & Failure Modes

- Path with query string (`/api/books?limit=5`) → match on the path part only.
- f-string path (`f"/api/books/{book.id}"`) → not a literal; skipped (P4). **OQ-TLINK-1** tracks partial f-string matching.
- `client.request("GET", "/path")` → recognized as verb-from-first-argument when it's a literal.
- Trailing slash mismatches (`/api/books/` vs `/api/books`) → normalized identically on both sides before matching, mirroring F01's path normalization.
- A test hits a path no route serves → no diagnostic in v1 (could be a legit 404 test); recorded as **OQ-TLINK-2**.

## 6. Open Questions & Decisions

- **OQ-TLINK-1** — f-string paths: match the literal prefix/suffix around interpolations? Deferred; measure how common they are in real suites first.
- **OQ-TLINK-2** — Opt-in `test/unknown-path` diagnostic for client calls matching no route. Tempting, but 404-tests make it false-positive-prone; revisit post-M4.

## Data Shapes & Code Map

```rust
// src/parsing/clients.rs — facts
pub struct ClientCall { pub verb: Method, pub path: String, pub string_range: Range,
                        pub client: ClientKind }
pub enum ClientKind { TestClient, HttpxClient, HttpxAsyncClient, FixtureNamed }   // recognition route

// src/linking.rs — match results into E07's test_refs
pub struct ClientCallSite { pub uri: Url, pub range: Range }
pub enum PathMatch { One(RouteId), Many(Vec<RouteId>), None }            // Many → editor picker
```

Files: `parsing/clients.rs` (test-file gating + extraction), `linking.rs` (concrete trie lookups, `test_refs` build). Query strings are stripped before matching; normalization shares `linking.rs`'s path helpers with F01.

## 7. Cross-References

- **Depends on:** [F01](F01-route-index.md) — resolved paths; [E07](../foundations/E07-data-model.md) — REQ-IDX-03 concrete trie lookup, `test_refs`.
- **Related:** [F02](F02-diagnostics.md) — catalog home if OQ-TLINK-2 ever lands; [E17](../foundations/E17-testing.md) — the bookshop's `tests/` fixture.

## 8. Changelog

- **2026-06-12** — Capability restructure: REQ-TLINK-03/04 moved out to [F13](F13-navigation.md), [F15](F15-code-lens.md), [F11](F11-completion.md).
- **2026-06-12** — Initial draft: shape-based client recognition, trie matching, bidirectional navigation, completion.
