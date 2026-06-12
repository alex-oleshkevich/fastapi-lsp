# F08 — Code Actions

> **Status:** Draft
>
> **Version:** 0.1   ·   **Last updated:** 2026-06-12
>
> **Purpose:** The edits the server offers, not just reports: quick fixes paired with the F02 diagnostics, and refactors — named dependency extraction, model creation, `Annotated` conversion, router extraction, test-stub generation.
>
> **Depends on:** [F02-diagnostics](F02-diagnostics.md), [F01-route-index](F01-route-index.md)   ·   **Related:** [F03-dependency-graph](F03-dependency-graph.md), [F04-test-linking](F04-test-linking.md), [F05-templates](F05-templates.md)

> Requirement tag: **ACT**

---

## 1. Purpose & Scope

A diagnostic that tells you what's wrong is half a feature; this spec is the other half. It also adds refactors that exist independently of any diagnostic — restructurings only this server can do safely, because they need the resolved-path and dependency indices.

This spec covers:

- Quick fixes attached to the [F02](F02-diagnostics.md) catalog
- Refactors: extract named dependency, create model, convert to/from `Annotated`, extract router, generate test stub, create missing dependency
- The correctness gate every action must pass

## 2. Non-Goals / Out of Scope

- Formatting or import-sorting of edited regions — the user's formatter owns style.
- Fixes for `route/duplicate` — the server can't know which route is the mistake; navigation via `relatedInformation` is the right tool.
- Type-inference-dependent edits (e.g. deriving model fields from a return statement) — P5.

## 3. Detailed Specification

### 3.1 The correctness gate

**REQ-ACT-01 — An action is offered only when its edit is provably correct.**

Constitution P4 applies to edits even harder than to squiggles: a wrong diagnostic wastes attention, a wrong edit corrupts code. Each action below states its gate; when the gate fails (two import candidates, ambiguous rename target), the action simply doesn't appear. Every action declares its proper kind (`quickfix`, `refactor.extract`, `refactor.rewrite`, `source`) so editors surface it in the right menu.

### 3.2 Quick fixes (kind: `quickfix`)

Each fix attaches to its diagnostic's range and ships in the same milestone as the check itself where noted.

| Diagnostic | Action | Edit | Gate |
|---|---|---|---|
| `di/depends-called` | Remove call — pass the function itself | `Depends(get_db())` → `Depends(get_db)` | Always (the range is exact). |
| `route/param-missing-arg` | Add parameter `book_id` to handler | Insert `book_id: str` into the signature (unannotated path params are `str` to FastAPI) | Signature has no `*args`/`**kwargs`. |
| `route/arg-missing-param` | Rename parameter to `{book_id}` | Rename the function parameter to the unbound path param | Exactly one unbound path param exists. |
| `route/arg-missing-param` | Add `/{book}` segment to path | Append the segment to the decorator path literal | Path is a literal (not `Unresolved`). |
| `tpl/missing-template` | Change to `book_list.html` | Replace the string with the near-miss match | F05's edit-distance ≤ 2 suggestion fired. |
| `tpl/missing-template` | Create `book_list.html` | Create the file under the highest-precedence template root | At least one template root exists. |
| `model/unknown-response-model` | Import `Book` from `app.models` | Add the import | Exactly one workspace model has that name. |
| `model/unknown-response-model` | Create model `BookOut` | The create-model refactor (§3.4), surfaced as a fix | §3.4's gate. |
| `route/shadowed` | Move route above `get_book` | Reorder the two handlers | Both handlers are in the same file. |
| `model/unknown-body-model` | Create model `BookCreate` | The create-model refactor (§3.4), surfaced as a fix | §3.4's gate. |

Was → become, one pair per text-edit fix (file-creating fixes noted inline):

```python
# di/depends-called → Remove call
db = Depends(get_db())                        # was
db = Depends(get_db)                          # becomes

# route/param-missing-arg → Add parameter        (path: /{book_id})
def get_book(): ...                           # was
def get_book(book_id: str): ...               # becomes

# route/arg-missing-param → Rename parameter     (path: /{book_id})
def get_book(book: int): ...                  # was
def get_book(book_id: int): ...               # becomes

# route/arg-missing-param → Add segment          (handler has book_id param)
@router.get("/books")                         # was
@router.get("/books/{book_id}")               # becomes

# tpl/missing-template → Change to near-miss
TemplateResponse(request, "book_lst.html")    # was
TemplateResponse(request, "book_list.html")   # becomes

# tpl/missing-template → Create template: creates templates/book_list.html; the string is untouched

# model/unknown-* → Import:  adds `from app.models import Book` at the top of the file

# route/shadowed → Move route above: the literal-path handler block moves above the
# param-path handler; no other text changes
```

### 3.3 Extract named dependency (kind: `refactor.extract`)

The FastAPI-docs pattern: a repeated `Annotated[Session, Depends(get_db)]` deserves a name.

**REQ-ACT-02 — Extracting an `Annotated` dependency creates a named alias and rewrites uses.**

With the cursor on an `Annotated[T, Depends(fn)]` annotation, the action **"Extract to named dependency `SessionDep`"**:

1. Inserts a module-level alias above the first use — name proposed as `{T}Dep` (`SessionDep` from `Session`), adjustable via the rename that editors apply to the inserted snippet:

```python
# was
@router.get("/{book_id}")
def get_book(book_id: int, db: Annotated[Session, Depends(get_db)]): ...

# becomes
SessionDep = Annotated[Session, Depends(get_db)]

@router.get("/{book_id}")
def get_book(book_id: int, db: SessionDep): ...
```

2. Replaces every *textually identical* `Annotated[T, Depends(fn)]` annotation in the same file with the alias — extracting one occurrence of a repeated pattern and leaving its twins is half a refactor.

A second variant, **"…and replace across workspace"**, extends step 2 to all files (a multi-file `WorkspaceEdit`), placing the alias in the package's `deps.py`/`dependencies.py` when one exists (imported where used) and at module level otherwise.

*Gate:* the annotation is a well-formed `Annotated[...]` with a `Depends(name)` where the name is bound ([F03 REQ-DI-02](F03-dependency-graph.md)); the proposed alias name is unbound in every edited scope.

### 3.4 Create model (kind: `quickfix` on diagnostics, `refactor.extract` on body params)

When a handler names a request/response model that doesn't exist yet, the server writes the stub where it belongs.

**REQ-ACT-03 — Create-model triggers on unknown body and response models.**

The action **"Create model `BookCreate`"** appears on:

- `response_model=BookCreate` naming an unknown symbol (alongside the `model/unknown-response-model` diagnostic), and
- a handler parameter annotation `book: BookCreate` where the name is CamelCase, in `model_index` nowhere, bound by no import in the file, and not a builtin (alongside `model/unknown-body-model`).

**REQ-ACT-04 — The target file resolves from imports first, convention second.**

1. **Imports first:** if the file already imports the name (`from app.schemas import BookCreate` — the import exists, the definition doesn't) or imports other names from a workspace module that holds Pydantic models, the class is appended to *that* module.
2. **Convention fallback:** otherwise the class goes into `schemas.py` in the referencing file's package — created if absent — and an import is added to the referencing file.

The generated stub is deliberately empty (fields would be guesses — P4):

```python
# was — app/routers/books.py (BookCreate unbound)
@router.post("/")
def create_book(book: BookCreate): ...

# becomes — app/routers/books.py
from app.schemas import BookCreate            # added

# becomes — app/schemas.py (created or appended)
from pydantic import BaseModel

class BookCreate(BaseModel):
    pass
```

`from pydantic import BaseModel` is added to the target module if missing. The new class lands in `model_index` at the next relink, clearing the diagnostic.

*Gate:* exactly one target module resolves; the name is genuinely unbound (an existing-but-unimported model gets the *import* fix from §3.2 instead, never a duplicate class).

### 3.5 Convert to `Annotated` style (kind: `refactor.rewrite`)

Offered on any default-style dependency parameter, with the reverse action on `Annotated` style. Purely syntactic, both directions always safe. Pairs with §3.3: convert, then extract.

```python
db: Session = Depends(get_db)                 # was
db: Annotated[Session, Depends(get_db)]       # becomes (+ `from typing import Annotated`)
```

### 3.6 Extract router (kind: `refactor.extract`)

On a selection of handlers whose resolved paths share a literal prefix: create the router, rewrite the decorators, add the include. The index proves the rewrite preserves every final URL — that proof is the gate, and it's the action only this server can offer.

```python
# was
@app.get("/books")
def list_books(): ...
@app.get("/books/{book_id}")
def get_book(book_id: int): ...

# becomes
router = APIRouter(prefix="/books")

@router.get("/")
def list_books(): ...
@router.get("/{book_id}")
def get_book(book_id: int): ...

app.include_router(router)
```

### 3.7 Generate test stub (kind: `source`)

On a handler: create or append a test stub in the matching `tests/` file. The new call links back via [F04](F04-test-linking.md) immediately, flipping the handler's CodeLens to `1 test reference`. *Gate:* the route's path is resolved and a `tests/` location is determinable (existing `test_<module>.py`, else `tests/` root).

```python
# becomes — tests/test_books.py (generated)
def test_get_book(client):
    response = client.get("/api/books/{book_id}")  # TODO: fill path params
    assert response.status_code == 200
```

### 3.8 Create missing dependency (kind: `quickfix`)

On a `Depends(get_pagination)` whose name binds to nothing: generate the stub in the package's `deps.py` (or above the handler when none exists) plus the import. No diagnostic accompanies this (an unbound name might be Pylance's territory), but offering help isn't asserting an error — the action may exist where the squiggle may not.

```python
# becomes — app/deps.py (generated)
def get_pagination():
    ...
```

## 4. Examples & Use Cases

You write `def create_book(book: BookCreate, db: Annotated[Session, Depends(get_db)])` in a fresh file. Two lightbulbs: *Create model `BookCreate`* — accept it, and `app/schemas.py` gains the stub while your file gains the import. Then *Extract to named dependency `SessionDep`* — accept, and the `Annotated` noise collapses to `db: SessionDep`, along with the identical annotation in `list_books` you'd forgotten about.

## 5. Edge Cases & Failure Modes

- Two `schemas.py` candidates (referencing file's package and app root) → imports-first rule usually disambiguates; if not, the action targets the referencing file's package (nearest wins, deterministically).
- Alias name collision (`SessionDep` already bound) → propose `SessionDep2`? No — gate fails, action withholds the workspace variant and offers same-file extraction with a numbered name only via the editor's rename flow. Never silently shadow.
- Create-model on a lowercase annotation (`book: book_create`) → not offered; the CamelCase gate is what separates "missing model" from "missing variable".
- Test-stub target file has a name collision (`test_get_book` exists) → append with a numeric suffix.

## 6. Open Questions & Decisions

- **OQ-ACT-1** — Should create-model infer fields when the handler body accesses attributes (`book.title`)? Attractive, but it's inference creep; revisit after dogfooding.
- **OQ-ACT-2** — Extract-router across files (handlers spread over modules). V1 is same-file selections only.
- **Decision** — Quick-fix batch (§3.2 rows 1–3) ships with M2 alongside their diagnostics; the rest is M8. Recorded in the [roadmap](../roadmap.md).

## Data Shapes & Code Map

```rust
// src/features/code_actions.rs
pub enum ActionId { RemoveDependsCall, AddPathParam, RenameParam, AddPathSegment,
                    FixTemplateName, CreateTemplate, ImportModel, CreateModel,
                    MoveRouteAbove, ExtractDependency { workspace: bool }, ConvertAnnotated { reverse: bool },
                    ExtractRouter, GenerateTestStub, CreateDependency, AddEnvKey, CopyEnvKey }
impl ActionId { pub fn kind(&self) -> CodeActionKind }                   // quickfix / refactor.* / source

pub enum Gate { Offer, Withhold }                                        // REQ-ACT-01: binary, no "maybe"
pub fn actions(state: &WorkspaceState, params: &CodeActionParams) -> Vec<CodeAction>;
```

Quick fixes read their inputs from the paired diagnostic's `data` payload ([F02](F02-diagnostics.md) REQ-DIAG-09) instead of re-running analysis. Files: `features/code_actions.rs` (dispatch + gates), one builder module per refactor family.

## 7. Cross-References

- **Depends on:** [F02](F02-diagnostics.md) — the diagnostics quick fixes attach to (resolves its OQ-DIAG-2); [F01](F01-route-index.md) — resolved paths gating extract-router and test stubs.
- **Related:** [F03](F03-dependency-graph.md) — name binding for §3.3/§3.8; [F04](F04-test-linking.md) — test-stub linking; [F05](F05-templates.md) — template fixes (resolves its OQ-TPL-1); [E07](../foundations/E07-data-model.md) — `model_index`.

## 8. Changelog

- **2026-06-12** — Initial draft: quick-fix table, extract named dependency, create model (imports-first targeting), `Annotated` conversion, extract router, test stubs.
