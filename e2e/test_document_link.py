"""E2e tests for document_link — template navigation (REQ-NAV-03)."""
from __future__ import annotations

from pathlib import Path

import pytest_lsp
from lsprotocol import types

from conftest import MAXIMAL_CAPS, wait_for_diagnostics

TPL_WORKSPACE = Path(__file__).parent / "fixtures" / "tpl_workspace"
APP_PY = TPL_WORKSPACE / "app.py"


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
            root_uri=TPL_WORKSPACE.as_uri(),
            workspace_folders=[
                types.WorkspaceFolder(uri=TPL_WORKSPACE.as_uri(), name="tpl_workspace")
            ],
        )
    )
    yield lsp_client
    await lsp_client.shutdown_session()


async def test_document_links_returned_for_template_strings(
    client: pytest_lsp.LanguageClient,
):
    """document_link must return links for template strings that exist in the index."""
    uri = _open(client, APP_PY)
    await wait_for_diagnostics(client, uri)

    result = await client.text_document_document_link_async(
        types.DocumentLinkParams(
            text_document=types.TextDocumentIdentifier(uri=uri),
        )
    )
    links = list(result) if result else []
    assert links, (
        "document_link must return at least one link for app.py "
        "(books.html and admin/dashboard.html are in the template index)"
    )


async def test_document_link_targets_template_file(
    client: pytest_lsp.LanguageClient,
):
    """Each link target must be a URI pointing to the actual template file."""
    uri = _open(client, APP_PY)
    await wait_for_diagnostics(client, uri)

    result = await client.text_document_document_link_async(
        types.DocumentLinkParams(
            text_document=types.TextDocumentIdentifier(uri=uri),
        )
    )
    links = list(result) if result else []
    assert links

    for link in links:
        assert link.target is not None, f"link must have a target URI: {link}"
        target = link.target if isinstance(link.target, str) else str(link.target)
        assert target.startswith("file://"), f"link target must be a file URI, got: {target!r}"
        assert ".html" in target, f"link must target an HTML template file, got: {target!r}"


async def test_document_links_include_books_html(
    client: pytest_lsp.LanguageClient,
):
    """books.html exists in the index — its link must appear in document_link results."""
    uri = _open(client, APP_PY)
    await wait_for_diagnostics(client, uri)

    result = await client.text_document_document_link_async(
        types.DocumentLinkParams(
            text_document=types.TextDocumentIdentifier(uri=uri),
        )
    )
    links = list(result) if result else []
    targets = [
        (link.target if isinstance(link.target, str) else str(link.target))
        for link in links
        if link.target is not None
    ]
    assert any("books.html" in t for t in targets), (
        f"expected a link to books.html, got targets: {targets}"
    )


async def test_document_links_include_nested_template(
    client: pytest_lsp.LanguageClient,
):
    """admin/dashboard.html exists in the index — its link must appear."""
    uri = _open(client, APP_PY)
    await wait_for_diagnostics(client, uri)

    result = await client.text_document_document_link_async(
        types.DocumentLinkParams(
            text_document=types.TextDocumentIdentifier(uri=uri),
        )
    )
    links = list(result) if result else []
    targets = [
        (link.target if isinstance(link.target, str) else str(link.target))
        for link in links
        if link.target is not None
    ]
    assert any("dashboard.html" in t for t in targets), (
        f"expected a link to admin/dashboard.html, got targets: {targets}"
    )


async def test_document_link_missing_template_has_no_link(
    client: pytest_lsp.LanguageClient,
):
    """book.html is NOT in the index — no link must be emitted for it."""
    uri = _open(client, APP_PY)
    await wait_for_diagnostics(client, uri)

    result = await client.text_document_document_link_async(
        types.DocumentLinkParams(
            text_document=types.TextDocumentIdentifier(uri=uri),
        )
    )
    links = list(result) if result else []
    targets = [
        (link.target if isinstance(link.target, str) else str(link.target))
        for link in links
        if link.target is not None
    ]
    # "book.html" (missing) must not appear — only "books.html" (existing) is allowed
    assert not any(t.endswith("book.html") for t in targets), (
        f"book.html is missing from index — no link must be emitted, got targets: {targets}"
    )
