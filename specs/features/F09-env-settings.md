# F09 — Env & Settings

> **Status:** Draft
>
> **Version:** 0.2   ·   **Last updated:** 2026-06-12
>
> **Purpose:** Environment-variable intelligence — hover, completion, goto, and diagnostics for `os.environ`/`os.getenv` lookups and Pydantic `BaseSettings` fields, backed by the workspace's `.env` files.
>
> **Depends on:** [E07-data-model](../foundations/E07-data-model.md), [E15-app-config](../foundations/E15-app-config.md)   ·   **Related:** [F02-diagnostics](F02-diagnostics.md), [F08-code-actions](F08-code-actions.md)

> Requirement tag: **ENV**

---

## 1. Purpose & Scope

An env key is a stringly-typed contract between code and deployment, and a Pydantic `Settings` field is the same contract wearing a type annotation. This spec makes both ends visible from either side — inspired by laravel-ls's `.env` support, adapted to Python idioms.

This spec covers:

- Parsing `.env` / `.env.example` into the env index
- Recognition of env lookups in Python code and of `BaseSettings` classes
- Hover, completion, goto definition, the `env/undefined-key` diagnostic, and two code actions

## 2. Non-Goals / Out of Scope

- Variables provided by the real deployment environment — invisible to static analysis by nature; the diagnostics below are worded so they never claim otherwise.
- Type-checking Settings fields or completing attribute access on a settings object — Pylance/ty's job (P5).
- Parsing shell scripts, Docker/compose files, or CI configs for env definitions. **OQ-ENV-2** tracks whether compose files earn an exception.

## 3. Detailed Specification

### 3.1 The env index

**REQ-ENV-01 — A configurable, discoverable set of env files feeds one index.**

The file set comes from [E15 REQ-CFG-05](../foundations/E15-app-config.md): the `env_files` config list (default `[".env", ".env.example"]`), union the paths discovered from code (REQ-ENV-08), union — opt-in — the server's process environment, whose entries are labeled `(process)` and carry no goto location. Each file is parsed (`KEY=value` lines, `#` comments, `export` prefixes tolerated) into `env_index`: key → value and location per source. All files are watched (REQ-ARCH-12). No sources at all means the index is empty and every absence-based feature below stays silent (P4).

**REQ-ENV-08 — Env file paths are discovered from the code that loads them.**

When code names its env file with a literal, that file joins the index — and the loading *instance* remembers which file it reads:

```python
config = Config(".env.prod")                                # starlette.config
model_config = SettingsConfigDict(env_file=".env.local")    # pydantic-settings
load_dotenv("conf/.env"); values = dotenv_values(".env")    # python-dotenv (bare load_dotenv() → ".env")
env = Env(); env.read_env(".env.test")                      # environs
```

Non-literal paths are ignored (P4). Lookups made *through a bound instance* — `config("KEY")`, `env.str("KEY")`, a Settings field whose class declares `env_file=` — resolve against that instance's file first, then the general set: hover and diagnostics on those sites know exactly which `.env` the runtime would read.

### 3.2 Recognized sites

**REQ-ENV-02 — Env lookups are recognized by shape, across the popular loaders.**

The key string literal in:

- `os.environ["KEY"]`, `os.environ.get("KEY"[, default])`, `os.getenv("KEY"[, default])`
- `<cfg>("KEY"[, cast=..., default=...])` where `<cfg>` is bound to a `starlette.config.Config(...)` (import-alias-aware, [E07 REQ-IDX-06](../foundations/E07-data-model.md))
- `<env>("KEY")` and `<env>.str/int/bool/float/list/dict/url/path("KEY"[, default])` where `<env>` is bound to an `environs.Env(...)`
- subscripts/`.get` on a dict bound to `dotenv_values(...)`

**REQ-ENV-03 — Settings classes bind fields to env keys.**

A class inheriting from a name bound to `BaseSettings` (pydantic-settings) is a settings model. Each field maps to its env key by pydantic's rules, resolved statically: the field name case-insensitively, overridden by `validation_alias=` or `alias=` string literals (`validation_alias` wins when both are present — it's the env-binding-specific one), prefixed by a literal `env_prefix` from `model_config = SettingsConfigDict(...)` or the legacy inner `Config` class. Non-literal prefixes/aliases mark the field's key `Unresolved`, excluding it from everything below.

One honesty note: pydantic-settings reads `.env` only when the class configures `env_file=` — it never loads it automatically. Our features describe what the *workspace files* contain, which is correct either way (Docker, direnv, and CI commonly load `.env` themselves); the hover wording in [F10](F10-hover.md) and the diagnostic wording in REQ-ENV-06 are both phrased around files, not runtime, for exactly this reason.

### 3.3 Diagnostic and code actions

**REQ-ENV-06 — `env/undefined-key` claims exactly what's provable.**

Severity Information, on the key string: `'APP_TIMEOUT' is not defined in workspace env files (.env, .env.example)`. It fires only when the lookup has no default (second argument to `get`/`getenv`, or a settings field with a default value). It deliberately does *not* claim the variable won't exist at runtime — deployment-provided vars are normal — which is why it's Information, not Warning, and why the message names the files checked. Detail row lives in the [F02 catalog](F02-diagnostics.md).

Well-known OS and CI variables never fire it. A built-in allowlist — `HOME`, `PATH`, `USER`, `PORT`, `HOSTNAME`, `PWD`, and the common CI vars — is suppressed by default; nobody puts `HOME` in `.env`. You can extend the list through [E15](../foundations/E15-app-config.md)'s `env.ignore` key.

**REQ-ENV-07 — Two code actions, per F08 conventions.**

- **Add `KEY` to `.env`** — a `WorkspaceEdit` appends the `KEY=` line. A `WorkspaceEdit` can't open a file, so the jump is a follow-up: the action's `Command` resolves server-side into `window/showDocument` targeting the new line, gated on the client's `window.showDocument` capability. Without that capability the edit still applies; you navigate manually. Offered on any `env/undefined-key`.
- **Copy `KEY` from `.env.example`** — appends the example's line to `.env`. Offered when the key exists only in `.env.example`. (The laravel-ls flow, ported.)

Both follow [F08](F08-code-actions.md) REQ-ACT-01's gates and `quickfix` kind, registered as `AddEnvKey` and `CopyEnvKey` in F08's action enum.

### 3.4 Capability surface

The masked-value hover lives in [F10](F10-hover.md) REQ-HOV-05 (including the secret-pattern rule); key completion in [F11](F11-completion.md) REQ-CPL-05; goto-to-`.env` in [F13](F13-navigation.md) REQ-NAV-01.

## 4. Examples & Use Cases

The bookshop grows `app/settings.py` with `class Settings(BaseSettings)` holding `database_url: str` and `mail_password: str`. Hover on `database_url` shows `DATABASE_URL = postgres://… (.env:3)`; hover on `mail_password` shows `••••••`. In a script you type `os.getenv("` — completion lists `DATABASE_URL`, `MAIL_PASSWORD`, plus `SMTP_HOST` which exists only in `.env.example`; picking it raises the Information diagnostic and the one-click copy action fixes it.

## 5. Edge Cases & Failure Modes

- Duplicate key within one file → last wins (dotenv semantics); goto targets the winning line.
- Multi-line / quoted values → value text is taken verbatim between the quotes; parse failures degrade that entry to `[unparsed]` without dropping the key.
- A settings field that's both aliased and prefixed → alias wins, prefix ignored (pydantic's rule).
- `.env` in a subdirectory (monorepo) → not indexed in v1; same boundary as [E15 OQ-CFG-1](../foundations/E15-app-config.md).

## 6. Open Questions & Decisions

- ~~**OQ-ENV-1**~~ — **Decision:** Masking is opt-out via `process_env_show_values = true` in [E15](../foundations/E15-app-config.md). Default stays masked. The toggle applies only to process-env entries (marked `(process)`); values from `.env` files are never shown.
- **OQ-ENV-2** — Index `docker-compose.yml` `environment:` blocks as a third definition source? Deferred until a real workspace wants it.

## Data Shapes & Code Map

```rust
// src/parsing/env.rs — facts
pub struct EnvLookup { pub key: String, pub string_range: Range, pub has_default: bool,
                       pub loader: LoaderKind, pub instance: Option<String> }
pub enum LoaderKind { OsEnviron, OsGetenv, StarletteConfig, Environs, DotenvValues, SettingsField }
pub struct EnvFileDecl { pub path: String, pub loader: LoaderKind, pub site: Location }  // REQ-ENV-08

// src/state.rs — index entries
pub struct EnvEntry { pub value: String, pub sources: Vec<EnvSource> }
pub enum EnvSource { File { uri: Uri, line: u32 }, Process }             // Process: no goto target
```

Files: `parsing/env.rs` (dotenv parsing, lookup + loader recognition, settings-field binding), `linking.rs` (per-instance file binding, index merge by source precedence). A file that fails to parse degrades its entries to `[unparsed]` values, never an error.

## 7. Cross-References

- **Depends on:** [E07](../foundations/E07-data-model.md) — `env_index`; [E15](../foundations/E15-app-config.md) — file watching, toggles, and the `env.ignore` key.
- **Related:** [F02](F02-diagnostics.md) — catalog row for `env/undefined-key`; [F08](F08-code-actions.md) — action conventions; the `AddEnvKey`/`CopyEnvKey` IDs live in F08's action enum, alongside their table rows; laravel-ls (prior art) — the `.env` feature set this adapts.

## 8. Changelog

- **2026-06-12** — Review pass: built-in OS/CI allowlist suppresses `env/undefined-key` by default, extensible via E15's `env.ignore`; "Add `KEY` to `.env`" respecified — the `WorkspaceEdit` appends, a capability-gated `window/showDocument` command opens the line; `AddEnvKey`/`CopyEnvKey` crosslinked to F08's action enum; `Url` → `Uri`.
- **2026-06-12** — Env-source overhaul: configurable `env_files` + opt-in process env (via E15 REQ-CFG-05); REQ-ENV-08 path discovery and per-instance file binding for starlette `Config`, pydantic-settings `env_file`, python-dotenv, environs; REQ-ENV-02 extended to those loaders' lookup shapes.
- **2026-06-12** — Doc-verification fixes: `validation_alias` precedence over `alias`; recorded that pydantic-settings loads `.env` only via explicit `env_file=` and why our file-based wording stays correct.
- **2026-06-12** — Capability restructure: REQ-ENV-04/05 moved out to [F10](F10-hover.md), [F11](F11-completion.md), [F13](F13-navigation.md).
- **2026-06-12** — Initial draft: env index, lookup/Settings recognition, masked hover, completion/goto, provable-claim diagnostic, two code actions.
