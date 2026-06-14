"""E2e tests for the hover feature (REQ-NAV-01, route cards, dep hover)."""
from __future__ import annotations

from pathlib import Path

import pytest_lsp
from lsprotocol import types

from conftest import MAXIMAL_CAPS, wait_for_diagnostics

BOOKSHOP = Path(__file__).parent / "fixtures" / "bookshop"
BOOKS_PY = BOOKSHOP / "app" / "routers" / "books.py"
DEPS_PY = BOOKSHOP / "app" / "deps.py"


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


async def test_hover_on_route_handler_returns_method_and_path(
    client: pytest_lsp.LanguageClient,
):
    """Hovering on a route handler name returns a markdown card with method and resolved path."""
    # books.py line 13: def list_books(db: DbDep):
    uri = _open(client, BOOKS_PY)
    _open(client, BOOKSHOP / "app" / "main.py")
    await wait_for_diagnostics(client, uri)

    result = await client.text_document_hover_async(
        types.HoverParams(
            text_document=types.TextDocumentIdentifier(uri=uri),
            position=types.Position(line=13, character=6),
        )
    )
    assert result is not None, "hover must return a result for a route handler"
    assert isinstance(result.contents, types.MarkupContent)
    assert result.contents.kind == types.MarkupKind.Markdown
    md = result.contents.value
    assert "GET" in md, f"hover card must mention method GET, got:\n{md}"
    assert "/books/" in md or "/api/books/" in md, (
        f"hover card must mention the route path, got:\n{md}"
    )


async def test_hover_on_route_handler_returns_markdown(
    client: pytest_lsp.LanguageClient,
):
    """Hover card for list_books must be non-empty markdown with bold method."""
    uri = _open(client, BOOKS_PY)
    _open(client, BOOKSHOP / "app" / "main.py")
    await wait_for_diagnostics(client, uri)

    result = await client.text_document_hover_async(
        types.HoverParams(
            text_document=types.TextDocumentIdentifier(uri=uri),
            position=types.Position(line=13, character=6),
        )
    )
    assert result is not None
    assert isinstance(result.contents, types.MarkupContent)
    md = result.contents.value
    # Card must have bold method name in markdown
    assert "**GET**" in md, f"hover card must use bold markdown for method, got:\n{md}"


async def test_hover_outside_handler_returns_none(
    client: pytest_lsp.LanguageClient,
):
    """Hovering on a non-route line (import) must return None or an empty result."""
    uri = _open(client, BOOKS_PY)
    await wait_for_diagnostics(client, uri)

    result = await client.text_document_hover_async(
        types.HoverParams(
            text_document=types.TextDocumentIdentifier(uri=uri),
            position=types.Position(line=0, character=5),  # 'from typing import ...'
        )
    )
    # The server may return None or a result with no useful content for non-route positions
    if result is not None:
        md = result.contents.value if isinstance(result.contents, types.MarkupContent) else ""
        assert "GET" not in md and "POST" not in md, (
            f"hover on import line must not return a route card, got:\n{md}"
        )
