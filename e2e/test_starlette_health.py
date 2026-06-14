"""E2e tests for the health/ fixture — raw Starlette routing (E17 §2.5, F06 §5)."""
from __future__ import annotations

from pathlib import Path

import pytest_lsp
from lsprotocol import types

from conftest import MAXIMAL_CAPS, MINIMAL_CAPS, wait_for_diagnostics

HEALTH = Path(__file__).parent / "fixtures" / "health"
HEALTH_APP = HEALTH / "app.py"


def _cfg(caps=MAXIMAL_CAPS) -> pytest_lsp.ClientServerConfig:
    return pytest_lsp.ClientServerConfig(server_command=["./target/debug/fastapi-lsp"])


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


# ── Fixtures ──────────────────────────────────────────────────────────────────

@pytest_lsp.fixture(config=_cfg(MAXIMAL_CAPS))
async def client_max(lsp_client: pytest_lsp.LanguageClient):
    """Maximal-capability client pointed at the health fixture."""
    await lsp_client.initialize_session(
        types.InitializeParams(
            capabilities=MAXIMAL_CAPS,
            root_uri=HEALTH.as_uri(),
            workspace_folders=[types.WorkspaceFolder(uri=HEALTH.as_uri(), name="health")],
        )
    )
    yield lsp_client
    await lsp_client.shutdown_session()


@pytest_lsp.fixture(config=_cfg(MINIMAL_CAPS))
async def client_min(lsp_client: pytest_lsp.LanguageClient):
    """Minimal-capability client pointed at the health fixture."""
    await lsp_client.initialize_session(
        types.InitializeParams(
            capabilities=MINIMAL_CAPS,
            root_uri=HEALTH.as_uri(),
            workspace_folders=[types.WorkspaceFolder(uri=HEALTH.as_uri(), name="health")],
        )
    )
    yield lsp_client
    await lsp_client.shutdown_session()


# ── Symbols ───────────────────────────────────────────────────────────────────

async def test_workspace_symbols_include_get_health(client_max: pytest_lsp.LanguageClient):
    """Workspace symbols include GET /health from the Starlette Route (F06 §5)."""
    _open(client_max, HEALTH_APP)
    await wait_for_diagnostics(client_max, HEALTH_APP.as_uri())

    result = await client_max.workspace_symbol_async(
        types.WorkspaceSymbolParams(query="health")
    )
    assert result is not None, "workspace_symbol must return a result"
    symbols = list(result) if result else []
    names = [s.name for s in symbols]
    assert any("health" in n.lower() for n in names), (
        f"Expected a symbol for /health route, got: {names}"
    )


async def test_workspace_symbols_include_mount_static(client_max: pytest_lsp.LanguageClient):
    """Workspace symbols include MOUNT /static from the StaticFiles mount (F06 §5)."""
    _open(client_max, HEALTH_APP)
    await wait_for_diagnostics(client_max, HEALTH_APP.as_uri())

    result = await client_max.workspace_symbol_async(
        types.WorkspaceSymbolParams(query="static")
    )
    assert result is not None
    symbols = list(result) if result else []
    names = [s.name for s in symbols]
    assert any("static" in n.lower() for n in names), (
        f"Expected a symbol for /static mount, got: {names}"
    )


# ── Diagnostics ───────────────────────────────────────────────────────────────

async def test_health_fixture_is_clean_maximal(client_max: pytest_lsp.LanguageClient):
    """health/app.py should have no diagnostics — it is a valid Starlette app (REQ-TST-02)."""
    uri = _open(client_max, HEALTH_APP)
    diags = await wait_for_diagnostics(client_max, uri)
    assert len(diags) == 0, f"health/app.py should be clean, got: {[d.code for d in diags]}"


async def test_health_fixture_is_clean_minimal(client_min: pytest_lsp.LanguageClient):
    """health/app.py is clean regardless of client capability profile (cross-capability parity)."""
    uri = _open(client_min, HEALTH_APP)
    diags = await wait_for_diagnostics(client_min, uri)
    assert len(diags) == 0, (
        f"health/app.py should be clean on minimal client, got: {[d.code for d in diags]}"
    )


# ── Cross-capability parity ───────────────────────────────────────────────────

async def test_diagnostics_publish_on_maximal(client_max: pytest_lsp.LanguageClient):
    """publishDiagnostics arrives for a maximal client (REQ-ARCH-10, cross-capability parity)."""
    uri = _open(client_max, HEALTH_APP)
    diags = await wait_for_diagnostics(client_max, uri)
    assert diags is not None


async def test_diagnostics_publish_on_minimal(client_min: pytest_lsp.LanguageClient):
    """publishDiagnostics arrives for a minimal client (REQ-ARCH-10, cross-capability parity)."""
    uri = _open(client_min, HEALTH_APP)
    diags = await wait_for_diagnostics(client_min, uri)
    assert diags is not None
