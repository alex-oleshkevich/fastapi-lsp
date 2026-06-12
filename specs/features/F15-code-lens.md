# F15 — Code Lens

> **Status:** Draft
>
> **Version:** 0.1   ·   **Last updated:** 2026-06-12
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

## 3. Edge Cases & Failure Modes

- The same handler reached by two mounts → one lens, counts merged (tests hit paths; the lens is about the handler).
- Editor without codeLens support (some Helix versions) → capability simply unused; F13 references carry the same information.

## Data Shapes & Code Map

```rust
// src/features/codelens.rs
pub fn code_lenses(state: &WorkspaceState, uri: &Url) -> Vec<CodeLens>;   // ranges only, data = LensData
pub fn resolve(state: &WorkspaceState, lens: CodeLens) -> CodeLens;       // fills count + command

#[derive(Serialize, Deserialize)]
struct LensData { route_id: RouteId }                                     // survives the resolve round-trip
```

Files: `features/codelens.rs`.

## 4. Cross-References

- **Depends on:** [F04](F04-test-linking.md) — `test_refs`; [E07](../foundations/E07-data-model.md) — stable IDs.
- **Related:** [F13](F13-navigation.md) — the same edges, navigation-shaped.

## 5. Changelog

- **2026-06-12** — Extracted from F04 §3.3 (lens part of REQ-TLINK-03) into a capability spec; added lazy resolve and stable anchoring.
