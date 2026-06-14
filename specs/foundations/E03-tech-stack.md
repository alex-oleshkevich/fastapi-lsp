# E03 — Tech Stack

> **Status:** Draft
>
> **Version:** 0.2   ·   **Last updated:** 2026-06-12
>
> **Purpose:** The baseline toolchain and dependencies, and the reasoning behind each pick.
>
> **Depends on:** [E01-architecture](E01-architecture.md)   ·   **Related:** [E17-testing](E17-testing.md)

---

## 1. Purpose & Scope

Rust 2024 edition, a deliberately short dependency list, and a Python-side dev harness. Versions below are floors, not pins — `Cargo.lock` does the pinning.

## 2. Detailed Specification

### 2.1 Runtime dependencies

Each crate earns its place; anything not on this list needs a recorded reason.

| Crate | Why |
|---|---|
| `tower-lsp-server` (≥ 0.23) | JSON-RPC framing and the `LanguageServer` trait. The actively maintained community fork of `tower-lsp` (the original has gone quiet); Biome, Oxc, and Harper ship on it. |
| `tokio` | Async runtime `tower-lsp` requires; also drives the pass-2 debounce timer. |
| `tree-sitter` + `tree-sitter-python` | The only parser (constitution P1). Error-tolerant trees are what make REQ-ARCH-07 possible. |
| `dashmap` | Concurrent maps for pass-1 state; feature reads never block pass-1 writes. |
| `arc-swap` | Atomic publication of the pass-2 `Linked` snapshot ([E07 §3.1](E07-data-model.md)); features load one consistent snapshot per request. |
| `notify` | Native file-watching fallback when the client can't register `workspace/didChangeWatchedFiles` ([E01 REQ-ARCH-12](E01-architecture.md)). |
| `clap` (derive) | The CLI surface — subcommands and flags for [F17](../features/F17-cli.md). |
| `serde` / `serde_json` | LSP payloads and config deserialization. |
| `toml` | `fastapi-lsp.toml` and `pyproject.toml` parsing ([E15](E15-app-config.md)). |
| `tracing` + `tracing-subscriber` | Structured logs, controlled by `RUST_LOG`, written to stderr (stdout belongs to the protocol). |

One type decision worth pinning here: the URI type throughout the codebase is `tower_lsp_server::ls_types::Uri` (a `fluent_uri` newtype), not `url::Url`. All path↔URI conversion goes through one helper module — nothing else touches it.

### 2.2 Build & dev toolchain

- **Rust 2024 edition**, stable channel.
- **`build.rs`** emits an ISO-8601 `BUILD_TIMESTAMP`, reported in `server_info.version` — answers "which build am I talking to?" during debugging.
- **Python e2e harness:** `uv` + `pytest` + **pytest-lsp** drive the protocol-level tests, with Neovim headless for the editor-in-the-loop layer ([E17](E17-testing.md)). No Python is needed to *build* or *run* the server, only to test it.

### 2.3 What we deliberately don't use

- **No Python-side analysis engine** (jedi, Pylance). The server must work without a Python environment present (P1, P5).
- **No incremental-graph crates.** Pass 2 rebuilds wholesale (E01 decision); a dependency for incremental recomputation is complexity we haven't earned. The same goes for salsa-style query engines — rust-analyzer needs one because type inference is global; our facts are per-file by design.
- **Not `async-lsp`.** Considered as the alternative to the tower-lsp lineage: it guarantees sequential notification handling and composable tower layers, but has a steeper API and fewer ergonomic helpers. We get the ordering guarantee we need by serializing document mutations per URI ourselves (E01 REQ-ARCH-08); revisit only if that proves insufficient.
- **Not a custom message loop on `lsp-types`.** A 2026 ecosystem sweep found no framework beyond the two above — what the biggest servers (Ruff, Tinymist, Biome) do instead is hand-roll the loop on raw `lsp-types`. That buys fine-grained concurrency control we don't need at this size; a framework keeps `server.rs` small.

## 3. Cross-References

- **Depends on:** [E01-architecture](E01-architecture.md) — the design these picks serve.
- **Related:** [E17-testing](E17-testing.md) — the uv/pytest harness; [E15-app-config](E15-app-config.md) — the TOML files parsed.

## 4. Changelog

- **2026-06-12** — v0.2: Review-fix batch — added `arc-swap` (pass-2 snapshot publication), `notify` (native file-watching fallback), and `clap` (F17 CLI) to the dependency table; pinned the URI type to `tower_lsp_server::ls_types::Uri` with one path↔URI helper module; ordering note updated to per-URI mutation serialization.
- **2026-06-12** — Switched `tower-lsp` → `tower-lsp-server` (maintained fork; original inactive) and recorded the `async-lsp` consideration, per LSP-ecosystem research. Touches [E01](E01-architecture.md).
- **2026-06-12** — Initial draft.
