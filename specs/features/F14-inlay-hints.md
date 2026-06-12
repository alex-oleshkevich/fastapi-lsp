# F14 — Inlay Hints

> **Status:** Draft
>
> **Version:** 0.1   ·   **Last updated:** 2026-06-12
>
> **Purpose:** What `textDocument/inlayHint` shows: the resolved full path next to handlers whose decorator alone is misleading.
>
> **Depends on:** [F01-route-index](F01-route-index.md)   ·   **Related:** [F10-hover](F10-hover.md)

> Requirement tag: **HINT**

---

## 1. Purpose & Scope

Inlay hints are rented pixels — each one must pay for itself. v1 has exactly one hint, and a rule for admitting more.

## 2. Detailed Specification

**REQ-HINT-01 — Resolved-path hints appear only where the decorator lies.**

A handler whose decorator path differs from its resolved path (any prefix applies) gets a hint after the decorator: `→ /api/books/{book_id}`. Routes with no prefix show nothing; unresolved routes show nothing. The hint's tooltip is the [F10](F10-hover.md) route card, and clicking it (where the editor supports `InlayHintLabelPart.location`) jumps to the `include_router` that contributed the prefix.

**REQ-HINT-02 — New hints need a "source is misleading" argument.**

A future hint is admitted only when the source text, read alone, gives a wrong impression that the hint corrects — the bar REQ-HINT-01 sets. Candidates on file: none.

## 3. Edge Cases & Failure Modes

- A router mounted twice → one hint per mount is wrong (they'd stack on one decorator); show `→ 2 mounts (hover for paths)` instead.

## Data Shapes & Code Map

```rust
// src/features/inlay_hints.rs
pub fn inlay_hints(state: &WorkspaceState, uri: &Url, range: Range) -> Vec<InlayHint>;
// label part carries the include site as a clickable location; tooltip = F10 route card
```

Files: `features/inlay_hints.rs`. One function, no local types.

## 4. Cross-References

- **Depends on:** [F01](F01-route-index.md) — resolved paths and chains.
- **Related:** [F10](F10-hover.md) — the tooltip content.

## 5. Changelog

- **2026-06-12** — Extracted from F01 §5.5 (REQ-ROUTE-08) into a capability spec; added the double-mount rule and the admission bar.
