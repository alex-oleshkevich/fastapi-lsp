# E02 — Folder Structure

> **Status:** Draft
>
> **Version:** 0.1   ·   **Last updated:** 2026-06-12
>
> **Purpose:** Where every kind of code lives, so the layout stays predictable as the crate grows.
>
> **Depends on:** [E01-architecture](E01-architecture.md)   ·   **Related:** [E17-testing](E17-testing.md)

---

## 1. Purpose & Scope

One crate, with the source tree mirroring the architecture's three layers: parsing (pass 1), linking (pass 2), and features (reads). This spec is the map.

## 2. Detailed Specification

The full tree. Each `parsing/` module owns one fact family; each `features/` module owns one LSP capability.

```
fastapi-lsp/
├── Cargo.toml
├── build.rs               # emits BUILD_TIMESTAMP into server_info.version
├── src/
│   ├── main.rs             # entry: lsp / check subcommands (F17)
│   ├── check.rs            # one-shot linter pipeline (F17)
│   ├── server.rs           # impl LanguageServer; process_file; debounce
│   ├── state.rs            # WorkspaceState + indices (E07)
│   ├── config.rs           # init options / fastapi-lsp.toml / pyproject (E15)
│   ├── linking.rs          # pass 2: router graph, dep graph, trie, test matching
│   ├── util.rs             # find_enclosing_call, position_in_range, …
│   ├── parsing/
│   │   ├── python.rs       # indicators, tree helpers
│   │   ├── routes.rs       # decorators, APIRouter, include_router, Route/Mount
│   │   ├── deps.rs         # Depends() references and dependency defs
│   │   ├── templates.rs    # template refs + url_for-in-template scan
│   │   ├── models.rs       # shallow Pydantic BaseModel index
│   │   ├── env.rs          # env files, lookups, loader discovery (F09)
│   │   ├── middleware.rs   # middleware registrations + stock table (F16)
│   │   └── clients.rs      # TestClient/httpx calls in tests
│   └── features/
│       ├── diagnostics.rs  completion.rs  goto.rs  references.rs
│       ├── hover.rs        symbols.rs     codelens.rs  inlay_hints.rs
│       ├── code_actions.rs document_link.rs
├── e2e/
│   ├── test_*.py           # one file per feature area
│   ├── client.py           # hand-rolled LspClient
│   └── fixtures/
│       ├── bookshop/       # the constitution's example app
│       └── health/         # raw-Starlette fixture
├── editors/zed/            # Zed extension (F07)
├── scripts/                # install-zed-extension.sh, release helpers
└── specs/                  # this suite
```

Three rules keep it honest:

- **Layering is one-way.** `features/` reads `state.rs`; it never calls into `parsing/` or `linking.rs`. Parsing never sees LSP types.
- **One capability per feature file.** A new LSP capability is a new file, not a new arm in an existing one.
- **Unit tests live beside their module** (`#[cfg(test)]`); anything that crosses the LSP boundary belongs in `e2e/`.

## 3. Cross-References

- **Depends on:** [E01-architecture](E01-architecture.md) — the layers this tree mirrors.
- **Related:** [E17-testing](E17-testing.md) — what goes in `e2e/`; [F07-editor-integration](../features/F07-editor-integration.md) — what goes in `editors/`.

## 4. Changelog

- **2026-06-12** — Initial draft.
