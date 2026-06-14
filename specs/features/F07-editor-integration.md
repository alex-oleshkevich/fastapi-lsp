# F07 — Editor Integration

> **Status:** Draft
>
> **Version:** 0.2   ·   **Last updated:** 2026-06-12
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

An extension under `editors/zed/`: `extension.toml` declaring the language server, plus a `scripts/install-zed-extension.sh` for local dev installs. The extension locates the binary on `PATH` first, then a configurable path. The extension also declares the template languages (HTML, Jinja) so the server attaches inside `.html` files — without that, template features never fire there.

One catch: declaring a language server in an extension does *not* make Zed run it beside the default Python server. You have to opt in by naming it in your settings — the `"..."` entry keeps the defaults running:

```jsonc
// ~/.config/zed/settings.json
{
  "languages": {
    "Python": { "language_servers": ["fastapi-lsp", "..."] }
  }
}
```

The README shows this snippet next to the install instructions; without it the extension installs but the server never starts.

[E15](../foundations/E15-app-config.md) initialization options travel through the same settings file, under `lsp`:

```jsonc
// ~/.config/zed/settings.json
{
  "lsp": {
    "fastapi-lsp": {
      "initialization_options": { "templates": ["app/templates"] }
    }
  }
}
```

### 3.2 Neovim

A README snippet — no plugin needed:

```lua
-- nvim-lspconfig
vim.lsp.config('fastapi_lsp', {
  cmd = { 'fastapi-lsp', '--stdio' },
  filetypes = { 'python', 'html', 'htmldjango' },
  root_markers = { 'pyproject.toml', '.git' },
})
vim.lsp.enable('fastapi_lsp')
```

The template filetypes are load-bearing: without them the server is never attached to `.html` buffers, and the template features ([F05](F05-templates.md), [F11](F11-completion.md), [F13](F13-navigation.md)) never fire inside templates.

### 3.3 Helix

A README snippet adding the server *alongside* the user's Python LSP — and to the template languages, for the same reason as the Neovim filetypes: a server not listed on `html`/`jinja` is never asked about templates.

```toml
# ~/.config/helix/languages.toml
[language-server.fastapi-lsp]
command = "fastapi-lsp"
args = ["--stdio"]

[[language]]
name = "python"
language-servers = ["pyright", "fastapi-lsp"]

[[language]]
name = "html"
language-servers = ["vscode-html-language-server", "fastapi-lsp"]

[[language]]
name = "jinja"
language-servers = ["fastapi-lsp"]
```

Order matters in Helix. It routes hover, goto-definition, and references to the *first* listed server that advertises the capability; only diagnostics, completion, code actions, and symbols merge across servers. With the type checker first (as above), its hover and goto stay primary — and our hover cards ([F10](F10-hover.md)) and string-goto ([F13](F13-navigation.md)) are simply unavailable in Helix, while diagnostics, completion, actions, and symbols still work. If you care more about framework navigation than type hovers, list `fastapi-lsp` first and take the reverse trade.

### 3.4 Packaging & distribution

Most Python developers install tools with `pip`, not `cargo` — so the distribution story has to meet them there. M7 ships these channels, all riding the same release workflow (`.github/workflows/release.yml`):

- **PyPI wheels via maturin** — `pip install fastapi-lsp` or `uvx fastapi-lsp`. This is the ruff model: the same binary shipped in a wheel, no Rust toolchain required.
- **Zed extension registry publication** — the local dev script (§3.1) is the dev loop, not the ship vehicle.
- **mason.nvim registry submission** — so Neovim users get it through `:Mason` like their other servers.
- **cargo-binstall-compatible release artifacts** — `cargo binstall fastapi-lsp` resolves prebuilt binaries instead of compiling.
- **Arch `PKGBUILD`** building from source, and plain `cargo install` from crates.io.

## 4. Edge Cases & Failure Modes

- The server must coexist with a primary Python LSP in every editor — it never registers capabilities that fight over formatting or full-file diagnostics ownership (its diagnostics are namespaced by `source: "fastapi-lsp"`).
- Binary missing from `PATH` → each editor surfaces its own error; the README troubleshooting section covers it.

## Data Shapes & Code Map

No Rust types — this feature is files:

```
editors/zed/extension.toml        # server registration: Python + template languages
editors/zed/src/                  # Zed extension glue (Rust→WASM, Zed's template)
scripts/install-zed-extension.sh  # local dev install
pyproject.toml                    # maturin wheel build (PyPI)
PKGBUILD                          # Arch package
.github/workflows/release.yml     # tag → build → release binaries + wheels
README.md                         # Neovim / Helix snippets + troubleshooting
```

## 5. Cross-References

- **Depends on:** [E01-architecture](../foundations/E01-architecture.md) — stdio transport, P2.
- **Related:** [roadmap](../roadmap.md) — M7; [E02](../foundations/E02-folder-structure.md) — `editors/`, `scripts/`.

## 6. Changelog

- **2026-06-12** — v0.2: Zed opt-in settings + init-options passing, Helix first-server routing trade-off, template filetypes in every snippet, distribution channels — PyPI wheels via maturin, Zed registry, mason.nvim, cargo-binstall artifacts.
- **2026-06-12** — Initial draft: Zed extension, Neovim/Helix snippets, PKGBUILD.
