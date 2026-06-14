# F05 — Templates

> **Status:** Draft
>
> **Version:** 0.2   ·   **Last updated:** 2026-06-12
>
> **Purpose:** Click-to-template and template-name completion for Jinja usage in Python code, the missing-template diagnostic, and `url_for` intelligence inside template files.
>
> **Depends on:** [E15-app-config](../foundations/E15-app-config.md), [E07-data-model](../foundations/E07-data-model.md)   ·   **Related:** [F02-diagnostics](F02-diagnostics.md), [F01-route-index](F01-route-index.md)

> Requirement tag: **TPL**

---

## 1. Purpose & Scope

`TemplateResponse("book_list.html", ...)` names a file the framework will load at runtime; this spec makes that name a live link at edit time. The scope is the *Python side* of the template boundary, plus one deliberate crossing: `url_for` references *inside* templates, because only the route index can validate them.

This spec covers:

- Pass-1 extraction of template references in Python code
- The template index (relative path → file) built from [E15](../foundations/E15-app-config.md)'s roots
- `url_for` site extraction from template files (REQ-TPL-06)
- The `tpl/missing-template` diagnostic

## 2. Non-Goals / Out of Scope

- Jinja language features inside template files — `{% extends %}` resolution, variable completion, Jinja syntax — a dedicated Jinja language server's job. The single exception is `url_for` (REQ-TPL-06), which needs *our* route index.
- Validating the *context dict* passed to a template against the variables it uses (would require full template parsing).

## 3. Detailed Specification

### 3.1 Recognized reference sites

**REQ-TPL-01 — Template strings are recognized by call shape.**

Pass 1 records the string literal in: `<env>.TemplateResponse(<name>, ...)` (both the modern `(request, name)` and legacy `(name, context)` argument orders) and `<env>.get_template(<name>)`. `<env>` is any name bound to a `Jinja2Templates(...)` or `jinja2.Environment(...)` construction in the file or its imports. Only string literals participate (P4).

### 3.2 The index

**REQ-TPL-02 — The template index maps relative paths to files, by root precedence.**

Pass 2 scans each template root from [E15 REQ-CFG-01](../foundations/E15-app-config.md) for files (any extension — Jinja templates aren't always `.html`), keyed by root-relative path with `/` separators. When two roots contain the same relative path, the higher-precedence root wins, matching Jinja's own first-match loader semantics. The index refreshes on file create/delete/rename under any root, via workspace file watching.

### 3.3 `url_for` inside templates

**REQ-TPL-06 — Template files are scanned for `url_for` sites.**

Jinja templates call `{{ url_for('get_book', book_id=book.id) }}` (and `{{ request.url_for(...) }}`); a renamed route breaks them silently. Files under template roots are scanned lexically for `url_for(`/`url_path_for(` occurrences whose first argument is a string literal — a narrow lexical pass, not a Jinja parse, because we only need these islands. Each site joins the same `url_for` facts as Python ones ([F01](F01-route-index.md) REQ-ROUTE-11), so it gets the full treatment: `url/unknown-name` and `url/param-mismatch` diagnostics in the template file, route-name completion inside the string, and goto-to-handler. Keyword arguments are only checked when they're literal Jinja arguments (`book_id=book.id` counts as *present*; its value is opaque — only names are compared).

> **Note:** The interactive features here — completion, goto, and hover on `url_for` — only reach a template the editor actually sends us. That requires attaching the server to the template filetypes: [F07](F07-editor-integration.md)'s configurations register `html`/`jinja` alongside Python. Pushed diagnostics for unopened templates are unaffected — they ride on the workspace scan.

### 3.4 Capability surface

Click-to-template (goto + document links) lives in [F13](F13-navigation.md) REQ-NAV-01/03; directory-aware path completion in [F11](F11-completion.md) REQ-CPL-04; the `tpl/missing-template` check (with its edit-distance suggestion) is cataloged in [F02](F02-diagnostics.md).

## 4. Examples & Use Cases

In `app/pages.py` you type `templates.TemplateResponse("` — completion lists `base.html` and `book_list.html`. You fat-finger `book_lst.html`; a warning underlines the string before you ever run the app. Fixed, the string turns into a link; ctrl-click drops you into the template — where `{{ url_for('get_bok') }}` is already wearing its own `url/unknown-name` squiggle.

## 5. Edge Cases & Failure Modes

- Reference written before any `Jinja2Templates(...)` exists in the workspace → not recognized (no `<env>` binding), so no features and no false diagnostic.
- Template root deleted while open → index refresh empties those entries; affected references re-diagnose on the next relink.
- Same env used across files (`from app.pages import templates`) → import-aware binding recognizes it, same rule as F03's edges.
- Case-sensitive filesystems vs case-typo (`Book_list.html`) → it's a miss; the diagnostic includes the nearest-name suggestion when edit distance ≤ 2.

## 6. Open Questions & Decisions

- **Decision (resolves OQ-TPL-1)** — The "create template" and "change to near-miss" quick fixes are specified in [F08-code-actions §3.2](F08-code-actions.md).

## Data Shapes & Code Map

```rust
// src/parsing/templates.rs — facts from Python files
pub struct TemplateRef { pub name: String, pub string_range: Range, pub env: Option<String> }
pub struct TemplateEnvDecl { pub name: String, pub directories: Vec<PathValue> }

// src/parsing/templates.rs — REQ-TPL-06 lexical scan of template files
pub struct TemplateUrlForSite { pub name: String, pub string_range: Range,
                                pub kwarg_names: Vec<String> }           // values opaque by design
```

The index itself is `template_index` (relative path → file `Uri`) in [E07](../foundations/E07-data-model.md)'s pass-2 snapshot; the `url_for` sites from the lexical scan land in E07's pass-1 `template_facts: DashMap<Uri, Vec<TemplateUrlForSite>>`. Files: `parsing/templates.rs` (both scans), `linking.rs` (root scan + precedence). A root that fails to read logs and yields an empty contribution — no error type crosses the module boundary.

## 7. Cross-References

- **Depends on:** [E15](../foundations/E15-app-config.md) — root resolution; [E07](../foundations/E07-data-model.md) — `template_index` and `template_facts`.
- **Related:** [F02](F02-diagnostics.md) — catalog conventions and the `url/*` checks REQ-TPL-06 feeds; [F07](F07-editor-integration.md) — attaching the server to template filetypes.

## 8. Changelog

- **2026-06-12** — Review pass: dropped `render_template` from REQ-TPL-01 (a Flask API; as a bare function it contradicted the env-binding rule); stated the F07 template-filetype dependency for in-template features; template-side facts aligned with E07's `template_facts`.
- **2026-06-12** — Added REQ-TPL-06 (`url_for` sites inside template files, lexical scan); template roots now come from the [E15](../foundations/E15-app-config.md) config schema.
- **2026-06-12** — Capability restructure: REQ-TPL-03/04 moved out to [F13](F13-navigation.md) and [F11](F11-completion.md).
- **2026-06-12** — Initial draft: reference shapes, root-precedence index, goto/completion/diagnostic.
