# F15 — Code Lens

> **Status:** Draft
>
> **Version:** 0.2   ·   **Last updated:** 2026-06-12
>
> **Purpose:** What `textDocument/codeLens` shows: test-reference counts above handlers.
>
> **Depends on:** [F04-test-linking](F04-test-linking.md)   ·   **Related:** [F13-navigation](F13-navigation.md)

> Requirement tag: **LENS**

---

## 1. Purpose & Scope

One lens in v1: how covered is this handler, and take me to the tests. Lenses occupy a whole editor line, so the noise bar is even higher than for inlay hints.

## 2. Detailed Specification

**REQ-LENS-01 — Test-reference lenses appear only when the count is non-zero.**

Each handler with at least one matched client call ([F04](F04-test-linking.md) REQ-TLINK-02) gets `▶ 2 test references` above its decorator, resolving to the location list. Handlers with zero matches show nothing — a `0 tests` lens on every handler is an accusation, not information.

**REQ-LENS-02 — Lenses resolve lazily and anchor on `RouteId`.**

The lens list returns ranges immediately; counts and commands fill in via `codeLens/resolve` so a large file's lens pass stays cheap. Anchoring on `RouteId` ([E07](../foundations/E07-data-model.md) REQ-IDX-01) keeps lenses stable across unrelated edits.

The resolved command is the server's own: `fastapi-lsp.showTestRefs`, advertised via `executeCommandProvider`. There is no portable client-side command for "show these locations" — `editor.action.showReferences` is a VS Code convention none of our editors implement, and clients send a lens's command back to the server through `workspace/executeCommand`. On execution the server opens the first matched call site via `window/showDocument` (gated on the `window.showDocument` client capability). Rendering the full multi-location list from a lens is a documented limitation; find-references on the handler ([F13](F13-navigation.md)) shows them all.

**REQ-LENS-03 — The server refreshes lenses after a relink.**

A new test changes counts in a routes file the user isn't editing, and clients cache lenses per document, re-requesting only on local edits. After a relink that changed counts, the server sends `workspace/codeLens/refresh` when the client advertises `workspace.codeLens.refreshSupport`.

## 3. Edge Cases & Failure Modes

- The same handler reached by two mounts → one lens, counts merged (tests hit paths; the lens is about the handler).
- Editor support is the narrowest of any capability in the suite, so be honest about it: **no** Helix version renders code lens, and Zed doesn't either; Neovim shows lenses only after opt-in setup (an autocmd calling `vim.lsp.codelens.refresh()` — the README documents it). [F13](F13-navigation.md) references are the primary surface for the same information; the lens is an enhancement where it renders.

## Data Shapes & Code Map

```rust
// src/features/codelens.rs
pub fn code_lenses(state: &WorkspaceState, uri: &Uri) -> Vec<CodeLens>;   // ranges only, data = LensData
pub fn resolve(state: &WorkspaceState, lens: CodeLens) -> CodeLens;       // fills count + command

#[derive(Serialize, Deserialize)]
struct LensData { route_id: RouteId }                                     // survives the resolve round-trip
```

Files: `features/codelens.rs`.

## 4. Cross-References

- **Depends on:** [F04](F04-test-linking.md) — `test_refs`; [E07](../foundations/E07-data-model.md) — stable IDs.
- **Related:** [F13](F13-navigation.md) — the same edges, navigation-shaped.

## 5. Changelog

- **2026-06-12** — v0.2 review pass: honest editor-support matrix (Neovim opt-in only; Zed and Helix render no lenses); lens command specified as server-side `fastapi-lsp.showTestRefs` via `executeCommandProvider` + `window/showDocument`; REQ-LENS-03 — `workspace/codeLens/refresh` after relink, capability-gated; `Uri` type in data shapes.
- **2026-06-12** — Extracted from F04 §3.3 (lens part of REQ-TLINK-03) into a capability spec; added lazy resolve and stable anchoring.
