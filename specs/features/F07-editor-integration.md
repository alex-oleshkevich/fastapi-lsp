# F07 — Editor Integration

> **Status:** Draft
>
> **Version:** 0.1   ·   **Last updated:** 2026-06-12
>
> **Purpose:** How the server reaches users: the Zed extension, Neovim and Helix configuration, and the Arch package.
>
> **Depends on:** [E01-architecture](../foundations/E01-architecture.md)   ·   **Related:** [roadmap](../roadmap.md)

---

## 1. Purpose & Scope

The server is editor-agnostic by construction (constitution P2); this spec covers the last mile per editor. Zed gets a real extension; Neovim and Helix get documented config; Arch gets a package. That's the whole surface.

## 2. Non-Goals / Out of Scope

- A VS Code extension — the official `fastapi-vscode` owns that editor (see [01-overview](../01-overview.md)).
- Editor-specific features. If a capability can't ship as standard LSP, it doesn't ship (P2).

## 3. Detailed Specification

### 3.1 Zed

An extension under `editors/zed/`: `extension.toml` declaring the language server for Python files alongside the primary Python LSP (Zed runs multiple servers per language), plus a `scripts/install-zed-extension.sh` for local dev installs. The extension locates the binary on `PATH` first, then a configurable path.

### 3.2 Neovim

A README snippet — no plugin needed:

```lua
-- nvim-lspconfig
vim.lsp.config('fastapi_lsp', {
  cmd = { 'fastapi-lsp', '--stdio' },
  filetypes = { 'python' },
  root_markers = { 'pyproject.toml', '.git' },
})
vim.lsp.enable('fastapi_lsp')
```

### 3.3 Helix

A README snippet adding the server *alongside* the user's Python LSP:

```toml
# ~/.config/helix/languages.toml
[language-server.fastapi-lsp]
command = "fastapi-lsp"
args = ["--stdio"]

[[language]]
name = "python"
language-servers = ["pyright", "fastapi-lsp"]
```

### 3.4 Packaging

An Arch `PKGBUILD` building from source. Other channels (crates.io binary install via `cargo install`, prebuilt release binaries on tags) ride the same release script and the release workflow (`.github/workflows/release.yml`).

## 4. Edge Cases & Failure Modes

- The server must coexist with a primary Python LSP in every editor — it never registers capabilities that fight over formatting or full-file diagnostics ownership (its diagnostics are namespaced by `source: "fastapi-lsp"`).
- Binary missing from `PATH` → each editor surfaces its own error; the README troubleshooting section covers it.

## Data Shapes & Code Map

No Rust types — this feature is files:

```
editors/zed/extension.toml        # server registration for Python files
editors/zed/src/                  # Zed extension glue (Rust→WASM, Zed's template)
scripts/install-zed-extension.sh  # local dev install
PKGBUILD                          # Arch package
.github/workflows/release.yml     # tag → build → release binaries
README.md                         # Neovim / Helix snippets + troubleshooting
```

## 5. Cross-References

- **Depends on:** [E01-architecture](../foundations/E01-architecture.md) — stdio transport, P2.
- **Related:** [roadmap](../roadmap.md) — M7; [E02](../foundations/E02-folder-structure.md) — `editors/`, `scripts/`.

## 6. Changelog

- **2026-06-12** — Initial draft: Zed extension, Neovim/Helix snippets, PKGBUILD.
