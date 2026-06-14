"""Extended diagnostic e2e tests — one test per remaining diagnostic code."""
from __future__ import annotations

from pathlib import Path

import pytest_lsp
from lsprotocol import types

from conftest import MAXIMAL_CAPS, wait_for_diagnostics

DIAG_FIXTURE = Path(__file__).parent / "fixtures" / "diagnostics"
DIAG_APP = DIAG_FIXTURE / "app.py"

DI_CYCLE = Path(__file__).parent / "fixtures" / "di_cycle"
DI_CYCLE_APP = DI_CYCLE / "app.py"

DI_OVERRIDE = Path(__file__).parent / "fixtures" / "di_override_unused"
DI_OVERRIDE_CONFTEST = DI_OVERRIDE / "conftest.py"

BOOKSHOP = Path(__file__).parent / "fixtures" / "bookshop"
BROKEN = BOOKSHOP / "app" / "routers" / "broken_routes.py"


def _find_diag(diags: list[types.Diagnostic], code: str) -> types.Diagnostic | None:
    for d in diags:
        if isinstance(d.code, str) and d.code == code:
            return d
    return None


def _open(lsp_client: pytest_lsp.LanguageClient, path: Path, version: int = 1) -> str:
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


# ── Client fixtures ───────────────────────────────────────────────────────────

@pytest_lsp.fixture(config=pytest_lsp.ClientServerConfig(
    server_command=["./target/debug/fastapi-lsp"],
))
async def client_diag(lsp_client: pytest_lsp.LanguageClient):
    await lsp_client.initialize_session(
        types.InitializeParams(
            capabilities=MAXIMAL_CAPS,
            root_uri=DIAG_FIXTURE.as_uri(),
            workspace_folders=[types.WorkspaceFolder(uri=DIAG_FIXTURE.as_uri(), name="diagnostics")],
        )
    )
    yield lsp_client
    await lsp_client.shutdown_session()


@pytest_lsp.fixture(config=pytest_lsp.ClientServerConfig(
    server_command=["./target/debug/fastapi-lsp"],
))
async def client_di_cycle(lsp_client: pytest_lsp.LanguageClient):
    await lsp_client.initialize_session(
        types.InitializeParams(
            capabilities=MAXIMAL_CAPS,
            root_uri=DI_CYCLE.as_uri(),
            workspace_folders=[types.WorkspaceFolder(uri=DI_CYCLE.as_uri(), name="di_cycle")],
        )
    )
    yield lsp_client
    await lsp_client.shutdown_session()


@pytest_lsp.fixture(config=pytest_lsp.ClientServerConfig(
    server_command=["./target/debug/fastapi-lsp"],
))
async def client_di_override(lsp_client: pytest_lsp.LanguageClient):
    await lsp_client.initialize_session(
        types.InitializeParams(
            capabilities=MAXIMAL_CAPS,
            root_uri=DI_OVERRIDE.as_uri(),
            workspace_folders=[types.WorkspaceFolder(uri=DI_OVERRIDE.as_uri(), name="di_override_unused")],
        )
    )
    yield lsp_client
    await lsp_client.shutdown_session()


@pytest_lsp.fixture(config=pytest_lsp.ClientServerConfig(
    server_command=["./target/debug/fastapi-lsp"],
))
async def client_bookshop(lsp_client: pytest_lsp.LanguageClient):
    await lsp_client.initialize_session(
        types.InitializeParams(
            capabilities=MAXIMAL_CAPS,
            root_uri=BOOKSHOP.as_uri(),
            workspace_folders=[types.WorkspaceFolder(uri=BOOKSHOP.as_uri(), name="bookshop")],
        )
    )
    yield lsp_client
    await lsp_client.shutdown_session()


# ── Tests ─────────────────────────────────────────────────────────────────────

async def test_env_undefined_key(client_diag: pytest_lsp.LanguageClient):
    """env/undefined-key: os.environ["UNDEFINED_SECRET_KEY"] with no .env file."""
    uri = _open(client_diag, DIAG_APP)
    diags = await wait_for_diagnostics(client_diag, uri)
    d = _find_diag(diags, "env/undefined-key")
    assert d is not None, f"expected env/undefined-key, got: {[x.code for x in diags]}"
    assert d.severity == types.DiagnosticSeverity.Information
    assert "UNDEFINED_SECRET_KEY" in d.message


async def test_route_duplicate_name(client_diag: pytest_lsp.LanguageClient):
    """route/duplicate-name: two routes share the same name kwarg."""
    uri = _open(client_diag, DIAG_APP)
    diags = await wait_for_diagnostics(client_diag, uri)
    d = _find_diag(diags, "route/duplicate-name")
    assert d is not None, f"expected route/duplicate-name, got: {[x.code for x in diags]}"
    assert d.severity == types.DiagnosticSeverity.Warning
    assert "shared_name" in d.message


async def test_route_shadowed(client_diag: pytest_lsp.LanguageClient):
    """route/shadowed: /{id} before /featured makes /featured unreachable."""
    uri = _open(client_diag, DIAG_APP)
    diags = await wait_for_diagnostics(client_diag, uri)
    d = _find_diag(diags, "route/shadowed")
    assert d is not None, f"expected route/shadowed, got: {[x.code for x in diags]}"
    assert d.severity == types.DiagnosticSeverity.Warning
    assert "shadow" in d.message.lower()


async def test_route_router_not_included(client_diag: pytest_lsp.LanguageClient):
    """route/router-not-included: router_unused declared but never app.include_router()'d."""
    uri = _open(client_diag, DIAG_APP)
    diags = await wait_for_diagnostics(client_diag, uri)
    d = _find_diag(diags, "route/router-not-included")
    assert d is not None, f"expected route/router-not-included, got: {[x.code for x in diags]}"
    assert d.severity == types.DiagnosticSeverity.Warning


async def test_url_param_mismatch(client_diag: pytest_lsp.LanguageClient):
    """url/param-mismatch: url_for("get_by_id", wrong_param=...) — path expects {id}."""
    uri = _open(client_diag, DIAG_APP)
    diags = await wait_for_diagnostics(client_diag, uri)
    d = _find_diag(diags, "url/param-mismatch")
    assert d is not None, f"expected url/param-mismatch, got: {[x.code for x in diags]}"
    assert d.severity == types.DiagnosticSeverity.Warning


async def test_route_arg_missing_param(client_diag: pytest_lsp.LanguageClient):
    """route/arg-missing-param: handler has 'extra' arg but path /simple has no {extra}."""
    uri = _open(client_diag, DIAG_APP)
    diags = await wait_for_diagnostics(client_diag, uri)
    d = _find_diag(diags, "route/arg-missing-param")
    assert d is not None, f"expected route/arg-missing-param, got: {[x.code for x in diags]}"


async def test_model_unknown_response_model(client_diag: pytest_lsp.LanguageClient):
    """model/unknown-response-model: response_model='UnknownModel' is not a known symbol."""
    uri = _open(client_diag, DIAG_APP)
    diags = await wait_for_diagnostics(client_diag, uri)
    d = _find_diag(diags, "model/unknown-response-model")
    assert d is not None, f"expected model/unknown-response-model, got: {[x.code for x in diags]}"
    assert "UnknownModel" in d.message


async def test_route_param_missing_arg(client_bookshop: pytest_lsp.LanguageClient):
    """route/param-missing-arg: path {book_id} not bound by handler in broken_routes.py."""
    uri = _open(client_bookshop, BROKEN)
    diags = await wait_for_diagnostics(client_bookshop, uri)
    d = _find_diag(diags, "route/param-missing-arg")
    assert d is not None, f"expected route/param-missing-arg, got: {[x.code for x in diags]}"
    assert d.severity == types.DiagnosticSeverity.Warning


async def test_di_cycle(client_di_cycle: pytest_lsp.LanguageClient):
    """di/cycle: dep_a → dep_b → dep_a forms a cycle detected by Tarjan's SCC."""
    uri = _open(client_di_cycle, DI_CYCLE_APP)
    diags = await wait_for_diagnostics(client_di_cycle, uri)
    d = _find_diag(diags, "di/cycle")
    assert d is not None, f"expected di/cycle, got: {[x.code for x in diags]}"
    assert d.severity == types.DiagnosticSeverity.Error
    assert "cycle" in d.message.lower()


async def test_di_override_unused(client_di_override: pytest_lsp.LanguageClient):
    """di/override-unused: dependency_overrides key refers to an unknown dep name."""
    uri = _open(client_di_override, DI_OVERRIDE_CONFTEST)
    diags = await wait_for_diagnostics(client_di_override, uri)
    d = _find_diag(diags, "di/override-unused")
    assert d is not None, f"expected di/override-unused, got: {[x.code for x in diags]}"
    assert d.severity == types.DiagnosticSeverity.Information
    assert "nonexistent_dep" in d.message
