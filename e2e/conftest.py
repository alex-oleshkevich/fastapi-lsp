"""Dual-profile pytest-lsp fixtures (REQ-TST-02)."""
from __future__ import annotations

import asyncio
import inspect
from pathlib import Path

import pytest
import pytest_lsp
from lsprotocol import types

# Path to the compiled binary (built by `cargo build` before running tests)
SERVER_BIN = Path(__file__).parent.parent / "target" / "debug" / "fastapi-lsp"

FIXTURES_DIR = Path(__file__).parent / "fixtures"


def _server_cmd() -> list[str]:
    return [str(SERVER_BIN)]


# ── Capability profiles ───────────────────────────────────────────────────────

MAXIMAL_CAPS = types.ClientCapabilities(
    general=types.GeneralClientCapabilities(
        position_encodings=[types.PositionEncodingKind.Utf8],
    ),
    text_document=types.TextDocumentClientCapabilities(
        publish_diagnostics=types.PublishDiagnosticsClientCapabilities(
            related_information=True,
            data_support=True,
        ),
        completion=types.CompletionClientCapabilities(
            completion_item=types.ClientCompletionItemOptions(
                snippet_support=False,
            ),
        ),
        code_lens=types.CodeLensClientCapabilities(
            dynamic_registration=True,
        ),
        inlay_hint=types.InlayHintClientCapabilities(
            dynamic_registration=True,
        ),
    ),
    workspace=types.WorkspaceClientCapabilities(
        did_change_watched_files=types.DidChangeWatchedFilesClientCapabilities(
            dynamic_registration=False,
        ),
        inlay_hint=types.InlayHintWorkspaceClientCapabilities(
            refresh_support=True,
        ),
        code_lens=types.CodeLensWorkspaceClientCapabilities(
            refresh_support=True,
        ),
        workspace_edit=types.WorkspaceEditClientCapabilities(
            resource_operations=["create", "rename", "delete"],
        ),
    ),
    window=types.WindowClientCapabilities(
        work_done_progress=True,
    ),
)

MINIMAL_CAPS = types.ClientCapabilities(
    text_document=types.TextDocumentClientCapabilities(
        publish_diagnostics=types.PublishDiagnosticsClientCapabilities(),
    ),
)


# ── Client fixtures ───────────────────────────────────────────────────────────

@pytest_lsp.fixture(config=pytest_lsp.ClientServerConfig(server_command=_server_cmd()))
async def client_maximal(lsp_client: pytest_lsp.LanguageClient):
    """LSP client with full capabilities."""
    await lsp_client.initialize_session(
        types.InitializeParams(
            capabilities=MAXIMAL_CAPS,
            root_uri=FIXTURES_DIR.as_uri(),
            workspace_folders=[
                types.WorkspaceFolder(uri=FIXTURES_DIR.as_uri(), name="fixture")
            ],
        )
    )
    yield lsp_client
    await lsp_client.shutdown_session()


@pytest_lsp.fixture(config=pytest_lsp.ClientServerConfig(server_command=_server_cmd()))
async def client_minimal(lsp_client: pytest_lsp.LanguageClient):
    """LSP client with bare capabilities (tests degraded paths)."""
    await lsp_client.initialize_session(
        types.InitializeParams(
            capabilities=MINIMAL_CAPS,
            root_uri=FIXTURES_DIR.as_uri(),
            workspace_folders=[
                types.WorkspaceFolder(uri=FIXTURES_DIR.as_uri(), name="fixture")
            ],
        )
    )
    yield lsp_client
    await lsp_client.shutdown_session()


# ── Helper: wait for diagnostics (never sleep — REQ-TST signal) ──────────────

async def wait_for_diagnostics(
    client: pytest_lsp.LanguageClient,
    uri: str,
    timeout: float = 10.0,
) -> list[types.Diagnostic]:
    """Block until publishDiagnostics arrives for uri. The server always sends
    one after didOpen (possibly empty), so this never deadlocks on a clean file.
    """
    async def _wait():
        while True:
            if uri in client.diagnostics:
                return client.diagnostics[uri]
            await asyncio.sleep(0.05)

    return await asyncio.wait_for(_wait(), timeout=timeout)


# ── Mark all tests asyncio by default ────────────────────────────────────────

def pytest_collection_modifyitems(items):
    for item in items:
        if inspect.iscoroutinefunction(item.function):
            item.add_marker(pytest.mark.asyncio)


def apply_text_edit(source: str, edit) -> str:
    """Apply a single TextEdit (or OneOf/InsertReplaceEdit) to source and return modified text."""
    if hasattr(edit, 'value'):
        edit = edit.value  # unwrap OneOf
    if hasattr(edit, 'replace'):  # InsertReplaceEdit
        r, new_text = edit.replace, edit.new_text
    else:
        r, new_text = edit.range, edit.new_text
    lines = source.splitlines(keepends=True)
    before = lines[r.start.line][:r.start.character]
    after = lines[r.end.line][r.end.character:]
    new_lines = lines[:r.start.line] + [before + new_text + after] + lines[r.end.line + 1:]
    return ''.join(new_lines)
