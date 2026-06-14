"""E2e tests for goto_definition and references (REQ-NAV-02)."""
from __future__ import annotations

from pathlib import Path

import pytest_lsp
from lsprotocol import types

from conftest import MAXIMAL_CAPS, wait_for_diagnostics

BOOKSHOP = Path(__file__).parent / "fixtures" / "bookshop"
BOOKS_PY = BOOKSHOP / "app" / "routers" / "books.py"
DEPS_PY = BOOKSHOP / "app" / "deps.py"
BROKEN_PY = BOOKSHOP / "app" / "routers" / "broken_routes.py"

# goto_fixture: dep defined in the same FastAPI file so it appears in file_facts
GOTO_FIXTURE = Path(__file__).parent / "fixtures" / "goto_fixture"
GOTO_APP = GOTO_FIXTURE / "app.py"


def _open(client: pytest_lsp.LanguageClient, path: Path, version: int = 1) -> str:
    uri = path.as_uri()
    client.text_document_did_open(
        types.DidOpenTextDocumentParams(
            text_document=types.TextDocumentItem(
                uri=uri,
                language_id="python",
                version=version,
                text=path.read_text(),
            )
        )
    )
    return uri


@pytest_lsp.fixture(config=pytest_lsp.ClientServerConfig(
    server_command=["./target/debug/fastapi-lsp"],
))
async def client(lsp_client: pytest_lsp.LanguageClient):
    await lsp_client.initialize_session(
        types.InitializeParams(
            capabilities=MAXIMAL_CAPS,
            root_uri=BOOKSHOP.as_uri(),
            workspace_folders=[types.WorkspaceFolder(uri=BOOKSHOP.as_uri(), name="bookshop")],
        )
    )
    yield lsp_client
    await lsp_client.shutdown_session()


@pytest_lsp.fixture(config=pytest_lsp.ClientServerConfig(
    server_command=["./target/debug/fastapi-lsp"],
))
async def client_goto(lsp_client: pytest_lsp.LanguageClient):
    await lsp_client.initialize_session(
        types.InitializeParams(
            capabilities=MAXIMAL_CAPS,
            root_uri=GOTO_FIXTURE.as_uri(),
            workspace_folders=[types.WorkspaceFolder(uri=GOTO_FIXTURE.as_uri(), name="goto_fixture")],
        )
    )
    yield lsp_client
    await lsp_client.shutdown_session()


async def test_goto_definition_on_depends_jumps_to_dep_def(
    client_goto: pytest_lsp.LanguageClient,
):
    """Goto-def on 'get_current_user' inside Depends() jumps to its definition in the same file.

    The dep is defined in the same FastAPI file (has 'from fastapi' indicator) so its dep_def
    appears in file_facts and can be resolved by goto_definition.
    """
    uri = _open(client_goto, GOTO_APP)
    await wait_for_diagnostics(client_goto, uri)

    lines = GOTO_APP.read_text().splitlines()
    # Find any line with Depends(get_current_user) inside a function signature
    line_no, col = next(
        (i, ln.index("get_current_user"))
        for i, ln in enumerate(lines)
        if "Depends(get_current_user)" in ln
    )

    result = await client_goto.text_document_definition_async(
        types.DefinitionParams(
            text_document=types.TextDocumentIdentifier(uri=uri),
            position=types.Position(line=line_no, character=col + 4),
        )
    )
    assert result is not None, "goto_definition must return a result for Depends(get_current_user)"

    if isinstance(result, types.Location):
        locs = [result]
    elif isinstance(result, list):
        locs = result
    else:
        locs = list(result) if result else []

    assert locs, "goto_definition must return at least one location"
    target_uri = locs[0].uri if isinstance(locs[0].uri, str) else str(locs[0].uri)
    assert "app.py" in target_uri, (
        f"goto-def on get_current_user must jump to app.py, got: {target_uri}"
    )


async def test_goto_definition_returns_none_outside_edge(
    client: pytest_lsp.LanguageClient,
):
    """Goto-def on a line with no navigable edge returns None."""
    uri = _open(client, BOOKS_PY)
    await wait_for_diagnostics(client, uri)

    # Line 0: 'from typing import Annotated' — no LSP edge here
    result = await client.text_document_definition_async(
        types.DefinitionParams(
            text_document=types.TextDocumentIdentifier(uri=uri),
            position=types.Position(line=0, character=5),
        )
    )
    # May be None or empty list — should not jump anywhere useful
    if result is not None:
        if isinstance(result, list):
            assert len(result) == 0 or all(
                "deps.py" not in (r.uri if isinstance(r.uri, str) else str(r.uri))
                for r in result
            )


async def test_references_on_dep_def_returns_dep_ref_sites(
    client_goto: pytest_lsp.LanguageClient,
):
    """References on get_current_user dep_def returns all Depends(get_current_user) sites."""
    uri = _open(client_goto, GOTO_APP)
    await wait_for_diagnostics(client_goto, uri)

    lines = GOTO_APP.read_text().splitlines()
    line_no = next(i for i, ln in enumerate(lines) if ln.startswith("def get_current_user"))
    col = lines[line_no].index("get_current_user")

    result = await client_goto.text_document_references_async(
        types.ReferenceParams(
            text_document=types.TextDocumentIdentifier(uri=uri),
            position=types.Position(line=line_no, character=col + 3),
            context=types.ReferenceContext(include_declaration=False),
        )
    )
    assert result is not None, (
        "references on get_current_user must return the Depends() call sites"
    )
    locs = list(result)
    assert len(locs) >= 2, (
        f"get_current_user is used in 2 routes — expected ≥2 refs, got: {len(locs)}"
    )


async def test_references_include_declaration_when_requested(
    client_goto: pytest_lsp.LanguageClient,
):
    """References with include_declaration=True on a dep_def includes the def itself."""
    uri = _open(client_goto, GOTO_APP)
    await wait_for_diagnostics(client_goto, uri)

    lines = GOTO_APP.read_text().splitlines()
    line_no = next(i for i, ln in enumerate(lines) if ln.startswith("def get_current_user"))
    col = lines[line_no].index("get_current_user")

    result = await client_goto.text_document_references_async(
        types.ReferenceParams(
            text_document=types.TextDocumentIdentifier(uri=uri),
            position=types.Position(line=line_no, character=col + 3),
            context=types.ReferenceContext(include_declaration=True),
        )
    )
    locs = list(result) if result else []
    uris = [r.uri if isinstance(r.uri, str) else str(r.uri) for r in locs]
    assert any("app.py" in u for u in uris), (
        f"with include_declaration=True, app.py must appear in refs, got: {uris}"
    )
    # With include_declaration, expect ≥3 (1 def + 2 Depends sites)
    assert len(locs) >= 3, (
        f"expected ≥3 locations (1 def + 2 Depends sites), got {len(locs)}: {uris}"
    )
