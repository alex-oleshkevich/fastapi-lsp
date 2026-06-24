"""E2E tests for # noqa inline suppression."""
from __future__ import annotations

from pathlib import Path

import pytest_lsp
from lsprotocol import types

from conftest import MAXIMAL_CAPS, wait_for_diagnostics

NOQA_FIXTURE = Path(__file__).parent / "fixtures" / "noqa_fixture"
NOQA_APP = NOQA_FIXTURE / "app.py"


def _find_diags(diags: list[types.Diagnostic], code: str) -> list[types.Diagnostic]:
    return [d for d in diags if isinstance(d.code, str) and d.code == code]


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


@pytest_lsp.fixture(config=pytest_lsp.ClientServerConfig(
    server_command=["./target/debug/fastapi-lsp"],
))
async def client(lsp_client: pytest_lsp.LanguageClient):
    await lsp_client.initialize_session(
        types.InitializeParams(
            capabilities=MAXIMAL_CAPS,
            root_uri=NOQA_FIXTURE.as_uri(),
            workspace_folders=[
                types.WorkspaceFolder(uri=NOQA_FIXTURE.as_uri(), name="noqa_fixture")
            ],
        )
    )
    yield lsp_client
    await lsp_client.shutdown_session()


async def test_bare_noqa_suppresses_diagnostic(client: pytest_lsp.LanguageClient):
    """Bare # noqa on a decorator line suppresses route/param-missing-arg for that route."""
    uri = _open(client, NOQA_APP)
    diags = await wait_for_diagnostics(client, uri)

    param_diags = _find_diags(diags, "route/param-missing-arg")
    # get_book has # noqa → suppressed; get_item has no # noqa → fires
    assert len(param_diags) == 1, (
        f"expected exactly 1 route/param-missing-arg (for get_item), got {len(param_diags)}: "
        f"{[(d.range.start.line, d.message) for d in param_diags]}"
    )
    # The surviving diagnostic must be for get_item (line 14, 0-indexed)
    assert param_diags[0].range.start.line == 14, (
        f"surviving diag should be on line 14 (get_item decorator), "
        f"got line {param_diags[0].range.start.line}"
    )


async def test_noqa_with_code_suppresses_only_matching(client: pytest_lsp.LanguageClient):
    """# noqa: route/param-missing-arg suppresses only that code, not others."""
    # Send a modified version of the file inline where # noqa is code-specific
    uri = NOQA_APP.as_uri()
    text = (
        "from fastapi import FastAPI\n"
        "app = FastAPI()\n"
        "@app.get('/{book_id}')  # noqa: route/param-missing-arg\n"
        "def get_book(title: str):\n"
        "    return {'title': title}\n"
    )
    client.text_document_did_open(
        types.DidOpenTextDocumentParams(
            text_document=types.TextDocumentItem(
                uri=uri,
                language_id="python",
                version=2,
                text=text,
            )
        )
    )
    diags = await wait_for_diagnostics(client, uri)
    param_diags = _find_diags(diags, "route/param-missing-arg")
    assert len(param_diags) == 0, (
        f"route/param-missing-arg should be suppressed by # noqa: route/param-missing-arg; "
        f"got {[(d.range.start.line, d.message) for d in param_diags]}"
    )
