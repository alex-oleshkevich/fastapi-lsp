"""E2e tests for inlay_hint — resolved path hints when prefix is applied (REQ-NAV-04)."""
from __future__ import annotations

from pathlib import Path

import pytest_lsp
from lsprotocol import types

from conftest import MAXIMAL_CAPS, wait_for_diagnostics

BOOKSHOP = Path(__file__).parent / "fixtures" / "bookshop"
BOOKS_PY = BOOKSHOP / "app" / "routers" / "books.py"
MAIN_PY = BOOKSHOP / "app" / "main.py"


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


async def test_inlay_hints_present_for_prefixed_routes(
    client: pytest_lsp.LanguageClient,
):
    """Inlay hints must appear in books.py because prefix '/api' is applied in main.py."""
    uri = _open(client, BOOKS_PY)
    _open(client, MAIN_PY)
    await wait_for_diagnostics(client, uri)

    source_lines = BOOKS_PY.read_text().splitlines()
    last_line = len(source_lines) - 1

    result = await client.text_document_inlay_hint_async(
        types.InlayHintParams(
            text_document=types.TextDocumentIdentifier(uri=uri),
            range=types.Range(
                start=types.Position(line=0, character=0),
                end=types.Position(line=last_line, character=0),
            ),
        )
    )
    hints = list(result) if result else []
    assert hints, (
        "inlay hints must appear in books.py because routes get '/api' prefix from main.py"
    )


async def test_inlay_hint_label_contains_resolved_path(
    client: pytest_lsp.LanguageClient,
):
    """Each inlay hint label must contain '→' and the resolved full path."""
    uri = _open(client, BOOKS_PY)
    _open(client, MAIN_PY)
    await wait_for_diagnostics(client, uri)

    source_lines = BOOKS_PY.read_text().splitlines()
    last_line = len(source_lines) - 1

    result = await client.text_document_inlay_hint_async(
        types.InlayHintParams(
            text_document=types.TextDocumentIdentifier(uri=uri),
            range=types.Range(
                start=types.Position(line=0, character=0),
                end=types.Position(line=last_line, character=0),
            ),
        )
    )
    hints = list(result) if result else []
    assert hints

    for hint in hints:
        label = hint.label if isinstance(hint.label, str) else hint.label[0].value
        assert "→" in label, f"hint label must contain '→', got: {label!r}"
        # Label is either "→ /api/books/..." (single mount) or "→ N mounts (hover for paths)"
        assert "/api/" in label or "/books" in label or "mount" in label, (
            f"hint label must mention the resolved path or mount count, got: {label!r}"
        )


async def test_inlay_hints_empty_for_file_without_prefix(
    client: pytest_lsp.LanguageClient,
):
    """No inlay hints for a file where decorator path == resolved path (no prefix applied)."""
    uri = _open(client, MAIN_PY)
    await wait_for_diagnostics(client, uri)

    source_lines = MAIN_PY.read_text().splitlines()
    last_line = len(source_lines) - 1

    result = await client.text_document_inlay_hint_async(
        types.InlayHintParams(
            text_document=types.TextDocumentIdentifier(uri=uri),
            range=types.Range(
                start=types.Position(line=0, character=0),
                end=types.Position(line=last_line, character=0),
            ),
        )
    )
    hints = list(result) if result else []
    # main.py itself has no route handlers (just include_router calls), so no hints
    assert hints == [], (
        f"main.py has no route handlers, expected no hints, got: {hints}"
    )
