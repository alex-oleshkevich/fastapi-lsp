"""Diagnostic e2e tests — one test per diagnostic code (E17 §2.5, F02)."""
from __future__ import annotations

from pathlib import Path

import pytest_lsp
from lsprotocol import types

from conftest import MAXIMAL_CAPS, MINIMAL_CAPS, wait_for_diagnostics

BOOKSHOP = Path(__file__).parent / "fixtures" / "bookshop"
BROKEN = BOOKSHOP / "app" / "routers" / "broken_routes.py"


def _find_diag(diags: list[types.Diagnostic], code: str) -> types.Diagnostic | None:
    for d in diags:
        if isinstance(d.code, str) and d.code == code:
            return d
    return None


# ── Fixtures ──────────────────────────────────────────────────────────────────

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
async def client_min(lsp_client: pytest_lsp.LanguageClient):
    await lsp_client.initialize_session(
        types.InitializeParams(
            capabilities=MINIMAL_CAPS,
            root_uri=BOOKSHOP.as_uri(),
            workspace_folders=[types.WorkspaceFolder(uri=BOOKSHOP.as_uri(), name="bookshop")],
        )
    )
    yield lsp_client
    await lsp_client.shutdown_session()


def _open(lsp_client: pytest_lsp.LanguageClient, path: Path, version: int = 1):
    uri = path.as_uri()
    lsp_client.text_document_did_open(
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


# ── Tests for implemented diagnostic codes ───────────────────────────────────

async def test_depends_called(client: pytest_lsp.LanguageClient):
    """di/depends-called: Depends(get_db()) — callable is invoked."""
    uri = _open(client, BROKEN)
    diags = await wait_for_diagnostics(client, uri)
    d = _find_diag(diags, "di/depends-called")
    assert d is not None, f"expected di/depends-called, got: {[x.code for x in diags]}"
    assert d.severity == types.DiagnosticSeverity.Error
    assert d.source == "fastapi-lsp"


async def test_route_duplicate(client: pytest_lsp.LanguageClient):
    """route/duplicate: two routes share method + path pattern."""
    uri = _open(client, BROKEN)
    diags = await wait_for_diagnostics(client, uri)
    d = _find_diag(diags, "route/duplicate")
    assert d is not None, f"expected route/duplicate, got: {[x.code for x in diags]}"
    assert d.severity == types.DiagnosticSeverity.Warning
    assert d.related_information is not None and len(d.related_information) > 0


async def test_url_unknown_name(client: pytest_lsp.LanguageClient):
    """url/unknown-name: url_for references non-existent route."""
    uri = _open(client, BROKEN)
    # Open main.py so the workspace scan covers all included routers
    _open(client, BOOKSHOP / "app" / "main.py")
    diags = await wait_for_diagnostics(client, uri)
    d = _find_diag(diags, "url/unknown-name")
    assert d is not None, f"expected url/unknown-name, got: {[x.code for x in diags]}"
    assert "no_such_route" in d.message


async def test_diagnostics_arrive_on_minimal_client(client_min: pytest_lsp.LanguageClient):
    """Diagnostics publish even to a minimal client (REQ-ARCH-10)."""
    uri = _open(client_min, BROKEN)
    diags = await wait_for_diagnostics(client_min, uri)
    assert diags is not None, "publishDiagnostics must arrive"
    d = _find_diag(diags, "di/depends-called")
    assert d is not None, "di/depends-called must appear regardless of client capabilities"


async def test_clean_file_gets_empty_diagnostics(client: pytest_lsp.LanguageClient):
    """A clean file receives an empty diagnostics list, not silence (REQ-ARCH-10)."""
    uri = _open(client, BOOKSHOP / "app" / "models.py")
    diags = await wait_for_diagnostics(client, uri)
    assert len(diags) == 0, f"models.py should have no diagnostics, got: {list(diags)}"


async def test_diagnostic_source_is_fastapi_lsp(client: pytest_lsp.LanguageClient):
    """All diagnostics carry source='fastapi-lsp' (REQ-DIAG-01)."""
    uri = _open(client, BROKEN)
    diags = await wait_for_diagnostics(client, uri)
    for d in diags:
        assert d.source == "fastapi-lsp", f"unexpected source '{d.source}' on {d.code}"
