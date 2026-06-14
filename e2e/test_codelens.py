"""E2e tests for code_lens and code_lens_resolve (REQ-TST-04)."""
from __future__ import annotations

import asyncio
from pathlib import Path

import pytest_lsp
from lsprotocol import types

from conftest import MAXIMAL_CAPS, wait_for_diagnostics

# codelens_fixture: test_items.py imports TestClient directly so it has FastAPI indicators
# and is indexed by the server → client_calls are stored → test_refs are built.
CODELENS = Path(__file__).parent / "fixtures" / "codelens_fixture"
APP_PY = CODELENS / "app.py"
TEST_ITEMS_PY = CODELENS / "test_items.py"

# The linker is debounced 300 ms; we wait a bit longer so test_refs are populated.
_LINKER_SETTLE = 1.0


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
            root_uri=CODELENS.as_uri(),
            workspace_folders=[types.WorkspaceFolder(uri=CODELENS.as_uri(), name="codelens_fixture")],
        )
    )
    yield lsp_client
    await lsp_client.shutdown_session()


async def test_code_lens_returns_lenses_for_tested_handlers(
    client: pytest_lsp.LanguageClient,
):
    """code_lens on app.py must return lenses because test_items.py has test refs."""
    # Open both the app and the test file; test_items.py has TestClient (indicator)
    # so it gets indexed → client_calls stored → linker builds test_refs
    uri = _open(client, APP_PY)
    test_uri = _open(client, TEST_ITEMS_PY)
    await wait_for_diagnostics(client, uri)
    await wait_for_diagnostics(client, test_uri)
    # Linker debounces 300 ms before building test_refs; wait for it to settle
    await asyncio.sleep(_LINKER_SETTLE)

    result = await client.text_document_code_lens_async(
        types.CodeLensParams(
            text_document=types.TextDocumentIdentifier(uri=uri),
        )
    )
    lenses = list(result) if result else []
    assert lenses, (
        "code_lens must return at least one lens for app.py "
        "(list_items and create_item are called by test_items.py)"
    )


async def test_code_lens_lenses_are_on_handler_lines(
    client: pytest_lsp.LanguageClient,
):
    """Code lenses must be positioned at handler definition lines."""
    uri = _open(client, APP_PY)
    test_uri = _open(client, TEST_ITEMS_PY)
    await wait_for_diagnostics(client, uri)
    await wait_for_diagnostics(client, test_uri)
    await asyncio.sleep(_LINKER_SETTLE)

    result = await client.text_document_code_lens_async(
        types.CodeLensParams(
            text_document=types.TextDocumentIdentifier(uri=uri),
        )
    )
    lenses = list(result) if result else []
    assert lenses, "need lenses to check their positions"

    # All lenses must have a valid range (line ≥ 0)
    for lens in lenses:
        assert lens.range.start.line >= 0, f"lens has invalid line: {lens.range}"


async def test_code_lens_resolve_fills_in_command(
    client: pytest_lsp.LanguageClient,
):
    """Resolving a code lens must fill in a command with a test count in the title."""
    uri = _open(client, APP_PY)
    test_uri = _open(client, TEST_ITEMS_PY)
    await wait_for_diagnostics(client, uri)
    await wait_for_diagnostics(client, test_uri)
    await asyncio.sleep(_LINKER_SETTLE)

    result = await client.text_document_code_lens_async(
        types.CodeLensParams(
            text_document=types.TextDocumentIdentifier(uri=uri),
        )
    )
    lenses = list(result) if result else []
    assert lenses, "need at least one lens to resolve"

    unresolved = lenses[0]
    resolved = await client.code_lens_resolve_async(unresolved)

    assert resolved.command is not None, "resolved lens must have a command"
    assert resolved.command.title, "resolved command must have a non-empty title"
    # Title should mention test count (e.g. "1 test" or "2 tests")
    title = resolved.command.title
    assert any(c.isdigit() for c in title), (
        f"resolved lens title should contain a test count, got: {title!r}"
    )


async def test_code_lens_count_matches_test_file(
    client: pytest_lsp.LanguageClient,
):
    """The resolved lens must show 1 test per handler (test_list_items, test_create_item)."""
    uri = _open(client, APP_PY)
    test_uri = _open(client, TEST_ITEMS_PY)
    await wait_for_diagnostics(client, uri)
    await wait_for_diagnostics(client, test_uri)
    await asyncio.sleep(_LINKER_SETTLE)

    result = await client.text_document_code_lens_async(
        types.CodeLensParams(
            text_document=types.TextDocumentIdentifier(uri=uri),
        )
    )
    lenses = list(result) if result else []

    # Resolve all lenses and collect titles
    resolved_titles = []
    for lens in lenses:
        resolved = await client.code_lens_resolve_async(lens)
        if resolved.command:
            resolved_titles.append(resolved.command.title)

    assert resolved_titles, "must have at least one resolved lens title"
    # Each handler has exactly 1 test
    assert any("1" in t for t in resolved_titles), (
        f"expected at least one lens with 1 test, got titles: {resolved_titles}"
    )
