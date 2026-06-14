# E17 — Testing

> **Status:** Draft
>
> **Version:** 0.2   ·   **Last updated:** 2026-06-12
>
> **Purpose:** The test categories, the e2e harness, and the fixture corpus every feature is verified against.
>
> **Depends on:** [E01-architecture](E01-architecture.md), [E02-folder-structure](E02-folder-structure.md)   ·   **Related:** [constitution](../constitution.md)

> Requirement tag: **TST**

---

## 1. Purpose & Scope

Two layers, mirroring the architecture: Rust unit tests prove extraction and linking on source snippets; Python e2e tests prove the protocol end to end against real fixture apps. Each spec's REQ items are the testable contract — every REQ is verified in one of these layers.

## 2. Detailed Specification

### 2.1 Unit tests (Rust)

Unit tests live beside their module and feed Python snippets straight to the extractors — no LSP, no files on disk. Each snippet carries a module path: import-aware binding ([E07 REQ-IDX-06](E07-data-model.md)) resolves names through module identity, so `extract` and `link` must know which file a snippet pretends to be.

```rust
// src/parsing/routes.rs  (#[cfg(test)])
#[test]
fn resolves_prefix_through_include() {
    let facts_main  = extract("app/main.py", r#"
from app.routers import books
app.include_router(books.router, prefix="/api")
"#);
    let facts_books = extract("app/routers/books.py", r#"
router = APIRouter(prefix="/books")
@router.get("/{book_id}")
def get_book(book_id: int): ...
"#);
    let linked = link(&[("app/main.py", facts_main), ("app/routers/books.py", facts_books)]);
    assert_eq!(linked.route("get_book").resolved_path, "/api/books/{book_id}");
}
```

**REQ-TST-01 — Every extractor and every linking rule has snippet-level unit tests**, including at least one broken-syntax case per extractor (constitution P3: partial code must yield partial facts, not panics).

### 2.2 E2E protocol tests (pytest-lsp)

`e2e/` drives the binary through **pytest-lsp** (from the lsp-devtools project) — a maintained pytest harness that behaves as a real LSP client: it spawns the server over stdio, performs the full `initialize` capability negotiation, runs the `didOpen`/`didChange` lifecycle, and awaits notifications like `publishDiagnostics`. The harness's client-side behavior is itself spec-conformant, so a passing test means a real editor would have seen the same answer.

```python
# e2e/test_symbols.py
@pytest.mark.asyncio
async def test_workspace_symbols_include_resolved_path(client: LanguageClient):
    syms = await client.workspace_symbol_async(WorkspaceSymbolParams(query="books"))
    assert any(s.name == "GET /api/books/{book_id} · get_book" for s in syms)
```

Tests open fixture files, wait for `publishDiagnostics` (the signal that pass 2 ran — never a fixed sleep), then call the method under test. This wait is safe even on clean fixtures: the server always publishes diagnostics for a newly opened file, empty array included ([E01 REQ-ARCH-10](E01-architecture.md)), so the harness can never deadlock waiting on a file with nothing wrong. One `test_*.py` per capability spec (F02, F08, F10–F15), so the test layout mirrors the spec layout.

**REQ-TST-02 — Every LSP capability a feature spec promises has at least one e2e test** exercising it through the real protocol against a fixture app. The client fixture is parameterized over two capability profiles — *maximal* (snippets, documentLink, codeLens resolve) and *minimal* (bare LSP) — so degraded paths (plain-text completions, no lenses) are tested, not just the happy editor.

**REQ-TST-03 — Position math is tested against non-ASCII source.** The offset-conversion module has unit tests with emoji and CJK content (multi-unit in UTF-16, multi-byte in UTF-8), and at least one e2e fixture file contains a route path and handler after a `# 日本語 🎉` comment line — the classic place encodings drift ([E01 REQ-ARCH-09](E01-architecture.md)).

### 2.3 Editor-in-the-loop tests (Neovim headless)

Protocol tests prove the server answers correctly; this layer proves a *real editor* renders those answers. Neovim is the automatable editor: `nvim --headless` embeds a production LSP client (`vim.lsp`) that is scriptable from Lua and runs in CI with no GUI.

**REQ-TST-04 — A Neovim smoke suite covers one scenario per capability.**

`e2e/editor/` holds a minimal init that registers the binary for Python files, plus Lua specs (driven by `mini.test` or `plenary`) that open the bookshop and assert through `vim.lsp.buf_request_sync`: hover returns the route card, completion inside `client.get("` contains a known path, goto-definition on a template string lands in the `.html` file, and the param-mismatch diagnostic appears with our `source`. One scenario per capability — depth lives in layer 2; this layer exists to catch integration failures protocol tests can't see (capability mismatches, encoding drift, our server fighting the primary Python LSP).

```bash
nvim --headless --noplugin -u e2e/editor/minimal_init.lua \
  -c "lua MiniTest.run()" -c "qa!"
```

**REQ-TST-05 — Protocol conduct has dedicated conformance tests.**

The [E01 §5.6](E01-architecture.md) rules are tested explicitly at layer 2: out-of-order-looking `didChange` bursts leave the document consistent (REQ-ARCH-08); a fixed diagnostic re-publishes as an empty array (REQ-ARCH-10); `initialize` returns before the scan finishes and `workDoneProgress` begin/end arrive (REQ-ARCH-11); `$/cancelRequest` on an in-flight request yields the `RequestCancelled` error, never silence; and file-watch events keep the index honest (REQ-ARCH-12) — creating a fixture file surfaces its routes in workspace symbols, deleting it removes them and clears its diagnostics, and a watcher change to an *open* file does not clobber the editor buffer.

The cancellation test is satisfiable only because request concurrency stays at the framework default (E01 REQ-ARCH-08) — a globally serialized server could never receive the cancel mid-request. Timing-sensitive assertions (scan, relink, hover latency) test against the concrete budgets in [E01 §8](E01-architecture.md).

### 2.4 Debugging aid

`lsp-devtools agent` (same project as pytest-lsp) proxies a real editor session against the binary and records the JSON-RPC traffic for inspection — the tool for "an editor misbehaves but the e2e suite is green". A note in the README, not a test layer.

### 2.5 The fixture corpus

Fixtures are real, runnable-in-principle apps under `e2e/fixtures/` — the constitution's example cast made literal:

- **`bookshop/`** — `FastAPI()` app, nested prefixed router, two-level `Depends` chain, Pydantic model, Jinja templates, and a `tests/` directory with `TestClient` calls. Exercises F01–F05.
- **`health/`** — raw Starlette app with `Route`, `Mount`, and `StaticFiles`. Exercises F06.
- **`factory/`** — the app built inside `def create_app():`, with function-local routers wired by settings. Proves extraction works at any nesting depth — the default shape for settings-dependent apps.
- **`srclayout/`** — a small app under `src/`, importable as `app.*` only through source-root inference ([E07 §3.4](E07-data-model.md)). Proves module-path resolution beyond the flat layout.
- Deliberately broken variants (a param mismatch, a duplicate route, a `Depends(get_db())`) live in `bookshop/` behind clearly named files, so diagnostic tests assert on real positions.

### 2.6 Commands

```bash
cargo test                                   # layer 1: extractors + linking
cargo build && uv run pytest e2e/ -v         # layer 2: pytest-lsp protocol suite
uv run pytest e2e/ -v -k "test_name"         # one e2e test
nvim --headless --noplugin -u e2e/editor/minimal_init.lua \
  -c "lua MiniTest.run()" -c "qa!"            # layer 3: editor smoke suite
RUST_LOG=debug ./target/debug/fastapi-lsp --stdio   # manual poking
lsp-devtools agent -- ./target/debug/fastapi-lsp --stdio  # record a real session
```

## 3. Edge Cases & Failure Modes

- E2e timing: never sleep; always await the `publishDiagnostics` notification before asserting (the debounce makes fixed sleeps flaky by design).
- Fixture drift: fixtures are referenced by specs (the bookshop *is* the example cast), so changing a fixture path means updating the specs that cite it.
- Neovim version drift in CI: pin the Neovim version in CI config; `vim.lsp` behavior changes between releases.

## 3.1 Open Questions & Decisions

- **OQ-TST-1** — Adopt gopls-style *marker tests* (assertions as `#@ hover(...)` comments inside fixture files, with golden-file updates) for diagnostic positions and hover content? Attractive for keeping expectations next to the code they test; decide after the first dozen layer-2 tests show how painful position literals are.

## 4. Cross-References

- **Depends on:** [E01-architecture](E01-architecture.md) — the two layers under test; [E02-folder-structure](E02-folder-structure.md) — where tests live.
- **Related:** every `F##` spec — its REQ items are the testable contract that REQ-TST-01/02 verify.

## 5. Changelog

- **2026-06-12** — REQ-TST-05 extended with file-watch conformance scenarios (REQ-ARCH-12).
- **2026-06-12** — E2E overhaul from tooling research: hand-rolled `LspClient` replaced by **pytest-lsp** (real-client semantics, dual capability profiles); new layer 3 Neovim-headless smoke suite (REQ-TST-04); protocol-conduct conformance tests (REQ-TST-05); `lsp-devtools agent` as debugging aid; OQ-TST-1 (gopls marker tests).
- **2026-06-12** — Added REQ-TST-03 (non-ASCII position-math tests) per [E01 REQ-ARCH-09](E01-architecture.md).
- **2026-06-12** — Initial draft: two test layers, fixture corpus, command set.
