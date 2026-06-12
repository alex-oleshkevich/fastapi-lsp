# fastapi-lsp — Specification Index

> **Status:** Living (continuously maintained)
>
> **Last updated:** 2026-06-12
>
> **Purpose:** The map of the whole specification suite — every spec, what it defines, when to load it, and how finished it is. Start here.

fastapi-lsp is a Rust language server giving FastAPI and Starlette codebases framework-semantic intelligence — route navigation, diagnostics, dependency-graph features, test linking, and template integration — in Zed, Neovim, and Helix. The suite is organized foundation-first: meta-docs set the rules, `E##` foundations define how the server is built, `F##` features define what it does.

**Foundation specs describe _how_ the app is built. Feature specs describe _what_ each feature does.** Features split into two kinds: **domain specs** (F01, F03–F06, F09) own indexing semantics — what's extracted and linked; **capability specs** (F02, F08, F10–F15) each own one LSP capability's user-facing behavior across all domains.

## Status legend

✅ Approved · 📝 In Review · ✏️ Draft · ♻️ Deprecated · ⛔ Rejected

## Tier 1 — Meta

| Spec | Purpose | Load this when | Status |
|---|---|---|---|
| [constitution](constitution.md) | Product principles (P1–P6), authoring conventions, the bookshop example cast | Writing or reviewing any spec | ✅ |
| [glossary](glossary.md) | Canonical definition of every domain term | A term is unclear | ✏️ |

## Tier 2 — Product

| Spec | Purpose | Load this when | Status |
|---|---|---|---|
| [01-overview](01-overview.md) | What fastapi-lsp is, in plain language | Onboarding to the project | ✏️ |
| [roadmap](roadmap.md) | Build order — milestones M0–M7 | Planning what to build next | ✏️ |

## Tier 3 — Foundations

| Spec | Purpose | Load this when | Status |
|---|---|---|---|
| [E01-architecture](foundations/E01-architecture.md) | Two-pass pipeline, process model, resilience rules | Understanding how it all fits | ✏️ |
| [E02-folder-structure](foundations/E02-folder-structure.md) | Source/test layout and layering rules | Adding any new module | ✏️ |
| [E03-tech-stack](foundations/E03-tech-stack.md) | Dependencies and toolchain, with reasons | Adding a dependency | ✏️ |
| [E07-data-model](foundations/E07-data-model.md) | `WorkspaceState`: facts and linked indices | Touching state or linking | ✏️ |
| [E15-app-config](foundations/E15-app-config.md) | Config schema, sources (init options / own file / pyproject), precedence | Touching config | ✏️ |
| [E17-testing](foundations/E17-testing.md) | Unit + e2e layers, fixture corpus, commands | Writing tests | ✏️ |

## Tier 4 — Features

| Spec | Purpose | Load this when | Status |
|---|---|---|---|
| [F00-template](features/F00-template.md) | Boilerplate for new feature specs | Starting a new feature | — |
| [F01-route-index](features/F01-route-index.md) | Route extraction, prefix resolution, route names | Anything touching routes | ✏️ |
| [F02-diagnostics](features/F02-diagnostics.md) | The diagnostic catalog and publishing rules | Adding or tuning a check | ✏️ |
| [F03-dependency-graph](features/F03-dependency-graph.md) | `Depends()` navigation and cycle detection | Touching the dep graph | ✏️ |
| [F04-test-linking](features/F04-test-linking.md) | Client-call ↔ route matching, CodeLens, path completion | Touching test features | ✏️ |
| [F05-templates](features/F05-templates.md) | Click-to-template, completion, missing-template check | Touching template features | ✏️ |
| [F06-starlette-routing](features/F06-starlette-routing.md) | Table-style `Route`/`Mount` extraction | Touching Starlette support | ✏️ |
| [F07-editor-integration](features/F07-editor-integration.md) | Zed extension, Neovim/Helix config, packaging | Shipping to an editor | ✏️ |
| [F08-code-actions](features/F08-code-actions.md) | Quick fixes for the F02 catalog; extract-dependency, create-model, extract-router refactors | Adding or tuning a code action | ✏️ |
| [F09-env-settings](features/F09-env-settings.md) | Env-var and Pydantic `BaseSettings` intelligence backed by `.env` files | Touching env/settings features | ✏️ |
| [F10-hover](features/F10-hover.md) | The hover cards: route, dependency, include, env | Touching hover | ✏️ |
| [F11-completion](features/F11-completion.md) | String-position completions: paths, names, templates, env keys | Touching completion | ✏️ |
| [F12-symbols](features/F12-symbols.md) | Routes as document/workspace symbols | Touching symbols | ✏️ |
| [F13-navigation](features/F13-navigation.md) | Goto definition, references, document links across all edges | Touching navigation | ✏️ |
| [F14-inlay-hints](features/F14-inlay-hints.md) | Resolved-path hints | Touching inlay hints | ✏️ |
| [F15-code-lens](features/F15-code-lens.md) | Test-reference lenses | Touching code lens | ✏️ |
| [F16-middleware](features/F16-middleware.md) | Middleware registration recognition, kwarg signatures, per-route chains | Touching middleware features | ✏️ |
| [F17-cli](features/F17-cli.md) | `lsp` / `check` subcommands; the linter mode | Touching the CLI | ✏️ |

## Deprecated

| Spec | Superseded by | Status |
|---|---|---|
| *none yet* | | |

## Rejected

| Spec | Why rejected | Status |
|---|---|---|
| *none yet* | | |

## Out of scope

Type checking (Pylance/ty), runtime tooling (OpenAPI generation, endpoint execution), VS Code packaging, and Jinja language features inside template files (except `url_for` — [F05 §3.3](features/F05-templates.md)). See [01-overview](01-overview.md) "What it isn't".

## Maintenance rule

When you author or change a spec, update its row here in the same edit. When a spec is **deprecated**, move it to `deprecated/` and list it above. When a proposal is **rejected**, move it to `rejected/` and list it.

## Changelog

- **2026-06-12** — Large batch: [F17-cli](features/F17-cli.md) (`lsp`/`check` subcommands); [E15](foundations/E15-app-config.md) v0.2 config system (init options / own file / pyproject, `[features]` toggles, env sources); [F09](features/F09-env-settings.md) loader support (starlette `Config`, pydantic `env_file`, python-dotenv, environs, opt-in process env); [F05](features/F05-templates.md) REQ-TPL-06 (`url_for` in templates); [F02](features/F02-diagnostics.md) `route/router-not-included`, `model/unknown-body-model`, REQ-DIAG-09 (related info + tags + data payloads), full worked-example gallery; [F08](features/F08-code-actions.md) was/become gallery; [F10](features/F10-hover.md) rendered popovers + applied-middleware line; [E07](foundations/E07-data-model.md) REQ-IDX-06 (import-alias binding); Data Shapes & Code Map sections on F01–F17; README + CI/release workflows; all sibling-LSP references removed.
- **2026-06-12** — Added [F16-middleware](features/F16-middleware.md) (kwarg completion via [F11](features/F11-completion.md) REQ-CPL-06) and [E01](foundations/E01-architecture.md) REQ-ARCH-12 (mandatory file watching); [E07](foundations/E07-data-model.md) and [E17](foundations/E17-testing.md) updated; [roadmap](roadmap.md) gains M10.
- **2026-06-12** — Doc-verification pass against official Starlette/FastAPI/pydantic-settings docs (31 claims checked, all spec-encoded behavior now sourced): path converters + `{p:path}` trie semantics ([F01](features/F01-route-index.md), [E07](foundations/E07-data-model.md)); Mount name namespacing ([F01](features/F01-route-index.md), [F02](features/F02-diagnostics.md)); `url_path_for`; bare `Depends()` as an edge ([F03](features/F03-dependency-graph.md)); `validation_alias` precedence and `env_file` honesty note ([F09](features/F09-env-settings.md)); directory sequences ([E15](foundations/E15-app-config.md)); implicit HEAD ([F06](features/F06-starlette-routing.md)).
- **2026-06-12** — [E17](foundations/E17-testing.md) e2e overhaul: pytest-lsp protocol layer, Neovim-headless editor layer (REQ-TST-04), conformance tests (REQ-TST-05); [E03](foundations/E03-tech-stack.md) toolchain updated.
- **2026-06-12** — Capability restructure: new specs [F10-hover](features/F10-hover.md), [F11-completion](features/F11-completion.md), [F12-symbols](features/F12-symbols.md), [F13-navigation](features/F13-navigation.md), [F14-inlay-hints](features/F14-inlay-hints.md), [F15-code-lens](features/F15-code-lens.md); domain specs F01/F03/F04/F05/F09 narrowed to indexing semantics with capability pointers.
- **2026-06-12** — Added [F09-env-settings](features/F09-env-settings.md) (from laravel-ls research); constitution v1.1 records the rejected runtime-introspection model; [E15](foundations/E15-app-config.md) gains OQ-CFG-2 (feature toggles); [E07](foundations/E07-data-model.md) gains `env_index`; [F02](features/F02-diagnostics.md) gains `env/undefined-key`; [roadmap](roadmap.md) gains M9.
- **2026-06-12** — Added [F08-code-actions](features/F08-code-actions.md) (resolves OQ-DIAG-2, OQ-TPL-1) and `url_for` support across [F01 §5.7](features/F01-route-index.md), [F02](features/F02-diagnostics.md) (`url/*` codes), [E07](foundations/E07-data-model.md) (`route_names`), [glossary](glossary.md), [roadmap](roadmap.md) (M8).
- **2026-06-12** — Initial suite: constitution, overview, roadmap, glossary, six foundations (E01–E03, E07, E15, E17), seven features (F01–F07).
