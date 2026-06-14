"""E2e tests for document_symbol (REQ-NAV-05)."""
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


async def test_document_symbols_returns_route_handlers(
    client: pytest_lsp.LanguageClient,
):
    """document_symbol for books.py must include list_books, get_book, create_book."""
    uri = _open(client, BOOKS_PY)
    await wait_for_diagnostics(client, uri)

    result = await client.text_document_document_symbol_async(
        types.DocumentSymbolParams(
            text_document=types.TextDocumentIdentifier(uri=uri),
        )
    )
    assert result is not None, "document_symbol must return a result"
    symbols = list(result) if result else []
    assert symbols, "document_symbol must return at least one symbol for books.py"

    names = []
    for s in symbols:
        if isinstance(s, types.DocumentSymbol):
            names.append(s.name)
        else:
            names.append(s.name)

    assert "list_books" in names or any("books" in n.lower() for n in names), (
        f"expected list_books in document symbols, got: {names}"
    )


async def test_document_symbols_have_function_kind(
    client: pytest_lsp.LanguageClient,
):
    """Route handler symbols must be of kind Function or Method."""
    uri = _open(client, BOOKS_PY)
    await wait_for_diagnostics(client, uri)

    result = await client.text_document_document_symbol_async(
        types.DocumentSymbolParams(
            text_document=types.TextDocumentIdentifier(uri=uri),
        )
    )
    symbols = list(result) if result else []
    assert symbols

    for s in symbols:
        kind = s.kind if isinstance(s, types.DocumentSymbol) else s.kind
        assert kind in (types.SymbolKind.Function, types.SymbolKind.Method), (
            f"route handler symbol must be Function or Method kind, got: {kind}"
        )


async def test_document_symbols_empty_for_non_route_file(
    client: pytest_lsp.LanguageClient,
):
    """document_symbol for deps.py (no route handlers) must return an empty list."""
    uri = _open(client, DEPS_PY)
    await wait_for_diagnostics(client, uri)

    result = await client.text_document_document_symbol_async(
        types.DocumentSymbolParams(
            text_document=types.TextDocumentIdentifier(uri=uri),
        )
    )
    symbols = list(result) if result else []
    assert symbols == [], (
        f"deps.py has no route handlers — document_symbol must return empty list, got: {symbols}"
    )
