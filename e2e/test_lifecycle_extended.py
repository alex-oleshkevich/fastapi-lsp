"""Extended lifecycle tests: did_save, did_close (REQ-ARCH-08..12)."""
from __future__ import annotations

import asyncio
from pathlib import Path

import pytest_lsp
from lsprotocol import types

from conftest import MAXIMAL_CAPS, wait_for_diagnostics

BOOKSHOP = Path(__file__).parent / "fixtures" / "bookshop"
BOOKS_PY = BOOKSHOP / "app" / "routers" / "books.py"
DEPS_PY = BOOKSHOP / "app" / "deps.py"


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


async def test_did_save_does_not_crash_server(client: pytest_lsp.LanguageClient):
    """did_save notification must be accepted without error (REQ-ARCH-08)."""
    uri = _open(client, BOOKS_PY)
    await wait_for_diagnostics(client, uri)

    # Send did_save — server should not crash or return an error
    client.text_document_did_save(
        types.DidSaveTextDocumentParams(
            text_document=types.TextDocumentIdentifier(uri=uri),
            text=BOOKS_PY.read_text(),
        )
    )
    # Allow the server to process the notification
    await asyncio.sleep(0.1)

    # Server must still respond to subsequent requests
    await client.text_document_hover_async(
        types.HoverParams(
            text_document=types.TextDocumentIdentifier(uri=uri),
            position=types.Position(line=13, character=6),
        )
    )
    # Result may be None or hover card — just verifying the server didn't crash
    # (a crash would raise an exception before we get here)


async def test_did_save_triggers_diagnostics_republish(client: pytest_lsp.LanguageClient):
    """did_save with text must trigger the debounced linker which republishes diagnostics."""
    # Use books.py (has FastAPI indicators → in file_facts → re-published after relink)
    uri = _open(client, BOOKS_PY)
    await wait_for_diagnostics(client, uri)

    # Clear the cached diagnostics so we can detect the re-publish
    client.diagnostics.pop(uri, None)

    # Send did_save WITH text so the server calls index_file_forced → bumps generation
    client.text_document_did_save(
        types.DidSaveTextDocumentParams(
            text_document=types.TextDocumentIdentifier(uri=uri),
            text=BOOKS_PY.read_text(),
        )
    )
    # Debounce linker waits 300 ms then publishes; allow up to 5 s
    diags = await wait_for_diagnostics(client, uri, timeout=5.0)
    assert diags is not None, (
        "publishDiagnostics must arrive after did_save with text "
        "(debounce linker republishes for all files in file_facts)"
    )


async def test_did_close_does_not_crash_server(client: pytest_lsp.LanguageClient):
    """did_close notification must be accepted without error (REQ-ARCH-08)."""
    uri = _open(client, BOOKS_PY)
    await wait_for_diagnostics(client, uri)

    client.text_document_did_close(
        types.DidCloseTextDocumentParams(
            text_document=types.TextDocumentIdentifier(uri=uri),
        )
    )
    await asyncio.sleep(0.1)

    # Open a different FastAPI file to verify the server still processes requests
    uri2 = _open(client, BOOKSHOP / "app" / "main.py")
    diags = await wait_for_diagnostics(client, uri2)
    assert diags is not None, "server must still publish diagnostics after did_close"


async def test_did_close_then_reopen_works(client: pytest_lsp.LanguageClient):
    """A file can be closed and re-opened; the server must republish diagnostics."""
    uri = _open(client, BOOKS_PY, version=1)
    await wait_for_diagnostics(client, uri)

    client.text_document_did_close(
        types.DidCloseTextDocumentParams(
            text_document=types.TextDocumentIdentifier(uri=uri),
        )
    )
    await asyncio.sleep(0.05)

    # Clear stale diagnostics
    client.diagnostics.pop(uri, None)

    # Re-open
    client.text_document_did_open(
        types.DidOpenTextDocumentParams(
            text_document=types.TextDocumentItem(
                uri=uri,
                language_id="python",
                version=2,
                text=BOOKS_PY.read_text(),
            )
        )
    )
    diags = await wait_for_diagnostics(client, uri, timeout=10.0)
    assert diags is not None, "publishDiagnostics must arrive after re-opening a closed file"
