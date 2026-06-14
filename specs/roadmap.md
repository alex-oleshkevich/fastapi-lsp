# Roadmap

> **Status:** Living (continuously maintained)
>
> **Last updated:** 2026-06-12
>
> **Purpose:** The build order — what ships in each milestone and why that sequence.
>
> **Related:** [01-overview](01-overview.md), [index](index.md)

---

## The shape of the build

The route index comes first because everything else reads from it: diagnostics check it, test linking matches against it, even templates hang off handlers it discovered. After that, each milestone is independently shippable — the server is useful from M1 onward and simply gets smarter.

The first public release is **M1 + M2 + M7**: a navigable route index, the linter, and a way to install it. The foundation specs move Draft → In Review when M0 implementation starts.

## Milestones

### M0 — Skeleton

A binary that initializes, scans a workspace, and answers a hover with "I parsed you". Proves the tower-lsp + tree-sitter plumbing and the e2e harness end to end.

- Crate scaffold per [E02-folder-structure](foundations/E02-folder-structure.md) and [E03-tech-stack](foundations/E03-tech-stack.md)
- `initialize`/`initialized`, `didOpen`/`didChange`/`didSave`, workspace scan
- pytest e2e harness with an `LspClient` fixture ([E17-testing](foundations/E17-testing.md))

### M1 — Route index & navigation ([F01](features/F01-route-index.md))

The foundation milestone: pass-1 extraction, pass-2 prefix linking, and the read-only features on top — symbols ([F12](features/F12-symbols.md)), hover ([F10](features/F10-hover.md)), inlay hints ([F14](features/F14-inlay-hints.md)), and the route parts of navigation ([F13](features/F13-navigation.md)). After M1 you can open the bookshop and search for `GET /api/books/{book_id}`. Note: capability specs are cross-cutting — each milestone implements its domain's slice of them.

### M2 — Diagnostics ([F02](features/F02-diagnostics.md))

The linter milestone: path-param mismatches, duplicates, shadowed routes, `Depends(fn())`. Ships second because every check reads the M1 index.

### M3 — Dependency graph ([F03](features/F03-dependency-graph.md))

`Depends()` becomes navigable: goto, references, usage hover, cycle detection.

### M4 — Test linking ([F04](features/F04-test-linking.md))

Test calls resolve to handlers via the path trie; CodeLens lands on handlers; path completion lands in client-call strings.

### M5 — Templates ([F05](features/F05-templates.md))

Template roots resolve from config / `Jinja2Templates(...)`; click-to-template, completion, missing-template diagnostics, and `url_for` intelligence inside template files.

### M6 — Raw Starlette ([F06](features/F06-starlette-routing.md))

Table-style `Route`/`Mount` extraction feeds the same indices, so M1–M4 features light up for Starlette apps with no further work.

### M7 — Editor packaging ([F07](features/F07-editor-integration.md))

Zed extension + install script, Neovim and Helix config snippets in the README, Arch PKGBUILD.

### M8 — Code actions ([F08](features/F08-code-actions.md))

The refactor milestone: extract named dependency, create model, `Annotated` conversion, extract router, test stubs. The simple quick fixes (`di/depends-called`, the param fixes) ship earlier, with M2, per F08's decision — M8 is everything beyond them.

### M9 — Env & settings ([F09](features/F09-env-settings.md))

`.env`-backed hover/completion/goto, `BaseSettings` field binding, the `env/undefined-key` diagnostic, and the two env quick fixes.

### M10 — Middleware ([F16](features/F16-middleware.md))

Registration recognition, the workspace + stock signature sources, kwarg completion (F11 REQ-CPL-06), and per-route applied chains for the hover card.

### M11 — CLI check mode ([F17](features/F17-cli.md))

The `lsp`/`check` subcommand split, code filters, text/json output, and the shared-engine parity tests.

Stretch: a `fastapi-lsp routes` subcommand that prints the resolved route table — the `flask routes` equivalent — recorded as OQ-CLI-2 in [F17](features/F17-cli.md).

## Sequencing rules

- M2–M5 each depend only on M1 and can be reordered if priorities shift.
- M6 depends on M1's index shape but on no other milestone.
- M7 can start any time after M1 produces a useful binary.
- M8 depends on M2 (its quick fixes attach to the diagnostic catalog) and on M3 (the extract-dependency gate cites F03's name binding); the test-stub action additionally wants M4.
- M9's hover/completion/goto need only M0 — env features touch no route machinery — but its `env/undefined-key` diagnostic and quick fixes ride on M2's publishing and F08 quick-fix plumbing.
- M10's kwarg completion needs only M0, but its applied-chain storage and the hover-card line need M1's route index and hover.
- M11 depends on M2 (it reuses the diagnostics engine wholesale).

## Changelog

- **2026-06-12** — Review pass: dependency claims corrected (M8 also needs M3; M9's diagnostic and quick fixes need M2; M10's applied chains and hover line need M1); first public release defined as M1 + M2 + M7; foundations move Draft → In Review at M0 start; `fastapi-lsp routes` stretch note added (OQ-CLI-2 in F17).
- **2026-06-12** — Added M11 (CLI check mode), depending on M2.
- **2026-06-12** — Added M10 (middleware), depending only on M0.
- **2026-06-12** — Added M9 (env & settings), depending only on M0.
- **2026-06-12** — Added M8 (code actions); noted the M2 quick-fix carve-out.
- **2026-06-12** — Initial roadmap: M0–M7.
