"""
Comprehensive e2e tests for template path completion (REQ-CPL-04) and
tpl/missing-template code actions (REQ-TPL-05).

Fixture: e2e/fixtures/tpl_workspace/
  templates/books.html            — top-level template file
  templates/admin/dashboard.html  — nested template (directory-level completion)
  app.py lines (0-indexed):
    10:  templates.TemplateResponse(request, "books.html")     ← correct
    15:  templates.TemplateResponse(request, "admin/dashboard.html") ← correct
    20:  templates.TemplateResponse(request, "book.html")      ← missing, near-miss

Column analysis for each string literal ("`" col 47 is the opening quote):
  "books.html"             node: col 47..59 (ts excl)  inner: col 48..58
  "admin/dashboard.html"   node: col 47..69 (ts excl)  inner: col 48..68
  "book.html"              node: col 47..58 (ts excl)  inner: col 48..57
"""
from __future__ import annotations

from pathlib import Path
from typing import Optional

import pytest_lsp
from lsprotocol import types

from conftest import MAXIMAL_CAPS, apply_text_edit, wait_for_diagnostics

TPL_WORKSPACE = Path(__file__).parent / "fixtures" / "tpl_workspace"
APP_PY = TPL_WORKSPACE / "app.py"

# ── Fixture ───────────────────────────────────────────────────────────────────

@pytest_lsp.fixture(config=pytest_lsp.ClientServerConfig(
    server_command=["./target/debug/fastapi-lsp"],
))
async def client(lsp_client: pytest_lsp.LanguageClient):
    await lsp_client.initialize_session(
        types.InitializeParams(
            capabilities=MAXIMAL_CAPS,
            root_uri=TPL_WORKSPACE.as_uri(),
            workspace_folders=[
                types.WorkspaceFolder(uri=TPL_WORKSPACE.as_uri(), name="tpl_workspace")
            ],
        )
    )
    yield lsp_client
    await lsp_client.shutdown_session()


def open_app(lsp_client: pytest_lsp.LanguageClient) -> str:
    uri = APP_PY.as_uri()
    lsp_client.text_document_did_open(
        types.DidOpenTextDocumentParams(
            text_document=types.TextDocumentItem(
                uri=uri,
                language_id="python",
                version=1,
                text=APP_PY.read_text(),
            )
        )
    )
    return uri


async def get_completions(
    lsp_client: pytest_lsp.LanguageClient,
    uri: str,
    line: int,
    character: int,
) -> Optional[types.CompletionList]:
    result = await lsp_client.text_document_completion_async(
        types.CompletionParams(
            text_document=types.TextDocumentIdentifier(uri=uri),
            position=types.Position(line=line, character=character),
            context=types.CompletionContext(
                trigger_kind=types.CompletionTriggerKind.Invoked
            ),
        )
    )
    if isinstance(result, types.CompletionList):
        return result
    if isinstance(result, list) and result:
        return types.CompletionList(is_incomplete=False, items=result)
    return None


async def get_code_actions(
    lsp_client: pytest_lsp.LanguageClient,
    uri: str,
    line: int,
    start_col: int,
    end_col: int,
    diagnostics: list[types.Diagnostic] | None = None,
) -> list[types.CodeAction]:
    result = await lsp_client.text_document_code_action_async(
        types.CodeActionParams(
            text_document=types.TextDocumentIdentifier(uri=uri),
            range=types.Range(
                start=types.Position(line=line, character=start_col),
                end=types.Position(line=line, character=end_col),
            ),
            context=types.CodeActionContext(
                diagnostics=diagnostics or [],
            ),
        )
    )
    actions = []
    for item in (result or []):
        if isinstance(item, types.CodeAction):
            actions.append(item)
    return actions


# ── Helpers ───────────────────────────────────────────────────────────────────

def get_text_edit(item: types.CompletionItem) -> types.TextEdit:
    te = item.text_edit
    assert te is not None, f"item '{item.label}' has no text_edit"
    if isinstance(te, types.TextEdit):
        return te
    if isinstance(te, types.InsertReplaceEdit):
        return types.TextEdit(range=te.replace, new_text=te.new_text)
    raise AssertionError(f"unexpected text_edit type: {type(te)}")


def find_action(actions: list[types.CodeAction], title_fragment: str) -> types.CodeAction:
    for a in actions:
        if title_fragment in a.title:
            return a
    titles = [a.title for a in actions]
    raise AssertionError(f"No action matching '{title_fragment}'. Available: {titles}")


def extract_text_edit_from_action(action: types.CodeAction) -> types.TextEdit:
    dc = action.edit and action.edit.document_changes
    assert dc, f"action '{action.title}' has no document_changes"
    edits = dc[0].edits if hasattr(dc[0], "edits") else []
    assert edits, f"action '{action.title}' has no text edits"
    first = edits[0]
    if hasattr(first, "new_text"):
        return first
    # OneOf: left is TextEdit
    return first.value


# ── Completion tests ─────────────────────────────────────────────────────────

async def test_completion_returns_file_item_for_known_template(
    client: pytest_lsp.LanguageClient,
):
    """Completion inside 'books.html' string returns a FILE-kind item."""
    uri = open_app(client)
    await wait_for_diagnostics(client, uri)

    # Cursor at col 52 — inside "books.html" on line 10
    result = await get_completions(client, uri, line=10, character=52)
    assert result is not None, "expected completion list, got None"

    labels = [i.label for i in result.items]
    assert "books.html" in labels, f"expected 'books.html' in completions, got: {labels}"

    item = next(i for i in result.items if i.label == "books.html")
    assert item.kind == types.CompletionItemKind.File, (
        f"expected FILE kind, got {item.kind}"
    )


async def test_completion_edit_produces_correct_line(
    client: pytest_lsp.LanguageClient,
):
    """Applying the completion TextEdit must yield a syntactically valid line.

    Source line 10: templates.TemplateResponse(request, "books.html")
    The edit range must cover only the inner content (no surrounding quotes), so
    applying it leaves the surrounding quotes intact.
    """
    uri = open_app(client)
    await wait_for_diagnostics(client, uri)

    source = APP_PY.read_text()
    result = await get_completions(client, uri, line=10, character=52)
    assert result is not None

    item = next(i for i in result.items if i.label == "books.html")
    te = get_text_edit(item)

    assert te.range.start.line == 10, f"expected edit on line 10, got {te.range.start.line}"
    assert '"' not in te.new_text, f"newText must not include quotes, got: {te.new_text!r}"

    result_line = apply_text_edit(source, te).splitlines()[10]
    assert '"books.html"' in result_line, (
        f"After applying edit, line must contain '\"books.html\"', got:\n  {result_line!r}"
    )


async def test_completion_for_nested_template_returns_file_item(
    client: pytest_lsp.LanguageClient,
):
    """Completion inside 'admin/dashboard.html' returns a FILE item with correct range."""
    uri = open_app(client)
    await wait_for_diagnostics(client, uri)

    # Cursor at col 55 — inside "admin/dashboard.html" on line 15
    result = await get_completions(client, uri, line=15, character=55)
    assert result is not None, "expected completions for nested template"

    labels = [i.label for i in result.items]
    assert "admin/dashboard.html" in labels, (
        f"expected 'admin/dashboard.html' in completions, got: {labels}"
    )

    item = next(i for i in result.items if i.label == "admin/dashboard.html")
    assert item.kind == types.CompletionItemKind.File

    te = get_text_edit(item)
    assert te.new_text == "admin/dashboard.html"
    assert '"' not in te.new_text, "newText must not include quotes"

    source = APP_PY.read_text()
    result_line = apply_text_edit(source, te).splitlines()[15]
    assert '"admin/dashboard.html"' in result_line, (
        f"After applying edit, line must contain the quoted path, got:\n  {result_line!r}"
    )


async def test_completion_outside_template_string_returns_nothing(
    client: pytest_lsp.LanguageClient,
):
    """Cursor outside a template string must produce no template completions."""
    uri = open_app(client)
    await wait_for_diagnostics(client, uri)

    # Col 10 is on the 'templates.' identifier, not inside any string
    result = await get_completions(client, uri, line=10, character=10)
    if result is not None:
        labels = [i.label for i in result.items]
        tpl_labels = [lbl for lbl in labels if ".html" in lbl]
        assert not tpl_labels, (
            f"should not offer template completions outside string, got: {tpl_labels}"
        )


async def test_completion_at_opening_quote_still_has_correct_edit_range(
    client: pytest_lsp.LanguageClient,
):
    """Cursor ON the opening quote (col 47) may trigger; if so, edit range must still be inner-only."""
    uri = open_app(client)
    await wait_for_diagnostics(client, uri)

    result = await get_completions(client, uri, line=10, character=47)
    if result is None:
        return  # acceptable: opening quote may or may not trigger
    for item in result.items:
        if ".html" in item.label:
            te = get_text_edit(item)
            assert te.range.start.character >= 48, (
                f"edit must not replace the opening quote — start was {te.range.start.character}"
            )
            assert '"' not in te.new_text, (
                f"newText must not contain quotes — got {te.new_text!r}"
            )


async def test_completion_just_past_closing_quote_behaviour(
    client: pytest_lsp.LanguageClient,
):
    """Cursor one character past the closing quote (col 59) should not produce template completions.

    Col 59 is ')' after '"books.html"' — clearly outside the string.
    """
    uri = open_app(client)
    await wait_for_diagnostics(client, uri)

    result = await get_completions(client, uri, line=10, character=59)
    if result is not None:
        html_items = [i for i in result.items if ".html" in i.label]
        assert not html_items, (
            f"should not offer .html completions at col 59 (past closing quote), got: {[i.label for i in html_items]}"
        )


# ── Code action tests ─────────────────────────────────────────────────────────

async def test_missing_template_diagnostic_fired(
    client: pytest_lsp.LanguageClient,
):
    """tpl/missing-template diagnostic must fire for 'book.html' (not in index)."""
    uri = open_app(client)
    diags = await wait_for_diagnostics(client, uri)

    missing = [d for d in diags if isinstance(d.code, str) and d.code == "tpl/missing-template"]
    assert missing, (
        f"expected tpl/missing-template diagnostic, got codes: {[d.code for d in diags]}"
    )
    diag = missing[0]
    assert "book.html" in diag.message, f"diagnostic message should mention path: {diag.message}"


async def test_change_to_action_replaces_inner_content_only(
    client: pytest_lsp.LanguageClient,
):
    """'Change to' code action TextEdit range must not touch the surrounding quotes.

    The string 'book.html' on line 20 has:
      opening quote at col 47, content at cols 48-56, closing quote at col 57.
    inner_range = [48, 57) — the edit must cover exactly this range.
    """
    uri = open_app(client)
    await wait_for_diagnostics(client, uri)

    # Request code actions on the string node range (col 47..58, line 20)
    actions = await get_code_actions(client, uri, line=20, start_col=47, end_col=58)

    change_action = find_action(actions, "Change to")
    assert "books.html" in change_action.title, (
        f"'Change to' should suggest 'books.html', got: {change_action.title!r}"
    )
    assert change_action.is_preferred, "Change to must be is_preferred=true"

    te = extract_text_edit_from_action(change_action)
    assert te.range.start.line == 20, f"edit on wrong line: {te.range.start.line}"
    assert te.new_text == "books.html", f"new text must be 'books.html', got {te.new_text!r}"
    assert '"' not in te.new_text, "newText must not contain quotes"

    source = APP_PY.read_text()
    result_line = apply_text_edit(source, te).splitlines()[20]
    assert '"books.html"' in result_line, (
        f"After applying edit, line must contain '\"books.html\"', got:\n  {result_line!r}"
    )
    assert result_line.count('"') == 2, (
        f"Must have exactly 2 quotes after edit, got:\n  {result_line!r}"
    )


async def test_change_to_action_result_is_syntactically_valid(
    client: pytest_lsp.LanguageClient,
):
    """Applying the 'Change to' edit via apply_text_edit must yield valid Python.

    Source line 20:  templates.TemplateResponse(request, "book.html")
    After edit → templates.TemplateResponse(request, "books.html")
    """
    uri = open_app(client)
    await wait_for_diagnostics(client, uri)

    source = APP_PY.read_text()
    actions = await get_code_actions(client, uri, line=20, start_col=47, end_col=58)
    change_action = find_action(actions, "Change to")
    te = extract_text_edit_from_action(change_action)

    result_line = apply_text_edit(source, te).splitlines()[20]

    assert '"books.html"' in result_line, (
        f"After edit the string literal must be '\"books.html\"', got:\n  {result_line!r}"
    )
    assert result_line.count('"') == 2, (
        f"Must have exactly 2 quotes after edit, got:\n  {result_line!r}"
    )


async def test_create_action_is_offered_when_can_create_files(
    client: pytest_lsp.LanguageClient,
):
    """'Create' code action must be offered when client declares resource_operations=[create].

    MAXIMAL_CAPS includes resource_operations=['create', 'rename', 'delete'].
    """
    uri = open_app(client)
    await wait_for_diagnostics(client, uri)

    actions = await get_code_actions(client, uri, line=20, start_col=47, end_col=58)
    create_action = find_action(actions, "Create 'book.html'")
    assert create_action is not None


async def test_create_action_targets_template_root(
    client: pytest_lsp.LanguageClient,
):
    """'Create' action must create the file under the auto-detected templates/ root."""
    uri = open_app(client)
    await wait_for_diagnostics(client, uri)

    actions = await get_code_actions(client, uri, line=20, start_col=47, end_col=58)
    create_action = find_action(actions, "Create 'book.html'")

    dc = create_action.edit and create_action.edit.document_changes
    assert dc, "Create action must have document_changes"
    op = dc[0]
    assert hasattr(op, "uri"), f"expected a CreateFile operation, got: {op!r}"
    target_uri: str = op.uri if isinstance(op.uri, str) else op.uri.as_uri()
    assert "templates/book.html" in target_uri, (
        f"Create must target templates/book.html, got: {target_uri!r}"
    )


async def test_no_code_action_for_existing_template(
    client: pytest_lsp.LanguageClient,
):
    """No tpl/missing-template code actions for 'books.html' which IS in the index."""
    uri = open_app(client)
    await wait_for_diagnostics(client, uri)

    actions = await get_code_actions(client, uri, line=10, start_col=47, end_col=59)
    tpl_actions = [a for a in actions if "Change to" in a.title or "Create '" in a.title]
    assert not tpl_actions, (
        f"should not offer tpl actions for an existing template, got: {[a.title for a in tpl_actions]}"
    )


async def test_code_action_not_triggered_for_cursor_outside_string(
    client: pytest_lsp.LanguageClient,
):
    """Code actions at a range that does not overlap 'book.html' must not include tpl fixes."""
    uri = open_app(client)
    await wait_for_diagnostics(client, uri)

    # Cursor far from line 20's string — on line 4 (app = FastAPI())
    actions = await get_code_actions(client, uri, line=4, start_col=0, end_col=5)
    tpl_actions = [a for a in actions if "Change to" in a.title or "Create '" in a.title]
    assert not tpl_actions, (
        f"should not offer tpl actions when cursor is not on a template string: "
        f"{[a.title for a in tpl_actions]}"
    )


async def test_change_to_action_does_not_appear_for_exact_match(
    client: pytest_lsp.LanguageClient,
):
    """No 'Change to' action when the path exists in the index (books.html)."""
    uri = open_app(client)
    await wait_for_diagnostics(client, uri)

    actions = await get_code_actions(client, uri, line=10, start_col=47, end_col=59)
    change_actions = [a for a in actions if "Change to" in a.title]
    assert not change_actions, (
        f"'Change to' should not appear when template exists in index, got: {[a.title for a in change_actions]}"
    )
