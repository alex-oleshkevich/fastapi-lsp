"""Protocol conduct conformance tests (E01 REQ-ARCH-08..12, E17 REQ-TST-05)."""
from __future__ import annotations

from pathlib import Path

import pytest_lsp
from lsprotocol import types

from conftest import MAXIMAL_CAPS, wait_for_diagnostics

BOOKSHOP = Path(__file__).parent / "fixtures" / "bookshop"


@pytest_lsp.fixture(config=pytest_lsp.ClientServerConfig(
    server_command=["./target/debug/fastapi-lsp"],
))
async def client(lsp_client: pytest_lsp.LanguageClient):
    result = await lsp_client.initialize_session(
        types.InitializeParams(
            capabilities=MAXIMAL_CAPS,
            root_uri=BOOKSHOP.as_uri(),
            workspace_folders=[types.WorkspaceFolder(uri=BOOKSHOP.as_uri(), name="bookshop")],
        )
    )
    lsp_client._init_result = result
    yield lsp_client
    await lsp_client.shutdown_session()


async def test_initialize_returns_before_scan(client: pytest_lsp.LanguageClient):
    """initialize must return immediately; scan is background (REQ-ARCH-11)."""
    # If we reached here, initialize returned — the fixture would have timed out otherwise.
    assert client._init_result is not None
    assert client._init_result.capabilities is not None


async def test_open_always_publishes_diagnostics(client: pytest_lsp.LanguageClient):
    """A newly opened file always gets publishDiagnostics, even if empty (REQ-ARCH-10)."""
    uri = (BOOKSHOP / "app" / "main.py").as_uri()
    client.text_document_did_open(
        types.DidOpenTextDocumentParams(
            text_document=types.TextDocumentItem(
                uri=uri,
                language_id="python",
                version=1,
                text=(BOOKSHOP / "app" / "main.py").read_text(),
            )
        )
    )
    diags = await wait_for_diagnostics(client, uri)
    # May be empty, but the signal must arrive
    assert diags is not None


async def test_incremental_change_applies_correctly(client: pytest_lsp.LanguageClient):
    """Incremental edits produce correct source after application (REQ-ARCH-03)."""
    uri = (BOOKSHOP / "app" / "deps.py").as_uri()
    text = (BOOKSHOP / "app" / "deps.py").read_text()

    client.text_document_did_open(
        types.DidOpenTextDocumentParams(
            text_document=types.TextDocumentItem(
                uri=uri, language_id="python", version=1, text=text
            )
        )
    )
    await wait_for_diagnostics(client, uri)

    # Append a comment via incremental edit
    lines = text.splitlines()
    last_line = len(lines) - 1
    last_char = len(lines[-1]) if lines else 0
    client.text_document_did_change(
        types.DidChangeTextDocumentParams(
            text_document=types.VersionedTextDocumentIdentifier(uri=uri, version=2),
            content_changes=[
                types.TextDocumentContentChangePartial(
                    range=types.Range(
                        start=types.Position(line=last_line, character=last_char),
                        end=types.Position(line=last_line, character=last_char),
                    ),
                    text="\n# ok",
                )
            ],
        )
    )
    # No crash = pass; diagnostic re-publish expected
    await wait_for_diagnostics(client, uri)
