"""
End-to-end tests for `fastapi-lsp routes` (REQ-CLI-01 routes subcommand).

Runs the compiled binary and asserts on exit code and output content.
"""
from __future__ import annotations

import json
import subprocess
from pathlib import Path


SERVER_BIN = Path(__file__).parent.parent / "target" / "debug" / "fastapi-lsp"
BOOKSHOP = Path(__file__).parent / "fixtures" / "bookshop"


def run_routes(*args: str, cwd: Path | None = None) -> subprocess.CompletedProcess:
    return subprocess.run(
        [str(SERVER_BIN), "routes", *args],
        capture_output=True,
        text=True,
        cwd=cwd or BOOKSHOP,
    )


# ── Exit codes ────────────────────────────────────────────────────────────────

def test_exit_code_0_on_success():
    """routes exits 0 for a valid workspace."""
    result = run_routes(str(BOOKSHOP))
    assert result.returncode == 0, (
        f"expected exit 0, got {result.returncode}\n"
        f"stdout: {result.stdout}\nstderr: {result.stderr}"
    )


def test_exit_code_2_for_nonexistent_path():
    """routes exits 2 for a path that does not exist."""
    result = run_routes("/nonexistent/path")
    assert result.returncode == 2


# ── Text output ───────────────────────────────────────────────────────────────

def test_text_output_contains_method():
    """Text output includes HTTP methods."""
    result = run_routes(str(BOOKSHOP))
    assert result.returncode == 0
    methods = {"GET", "POST", "PUT", "PATCH", "DELETE"}
    found = any(m in result.stdout for m in methods)
    assert found, f"expected at least one HTTP method in output:\n{result.stdout}"


def test_text_output_contains_path():
    """Text output includes route paths starting with /."""
    result = run_routes(str(BOOKSHOP))
    assert result.returncode == 0
    lines = [ln for ln in result.stdout.splitlines() if ln.strip()]
    assert any("/" in line for line in lines), (
        f"expected route path in output:\n{result.stdout}"
    )


def test_text_output_contains_source_location():
    """Text output includes a file:line source location."""
    result = run_routes(str(BOOKSHOP))
    assert result.returncode == 0
    lines = [ln for ln in result.stdout.splitlines() if ln.strip()]
    assert any(".py:" in line for line in lines), (
        f"expected file:line reference in output:\n{result.stdout}"
    )


# ── JSON output ───────────────────────────────────────────────────────────────

def test_json_output_is_ndjson():
    """--format json produces one JSON object per line."""
    result = run_routes(str(BOOKSHOP), "--format", "json")
    assert result.returncode == 0
    lines = [ln for ln in result.stdout.splitlines() if ln.strip()]
    assert len(lines) > 0, "expected at least one route"
    for line in lines:
        obj = json.loads(line)
        assert "method" in obj
        assert "path" in obj
        assert "name" in obj
        assert "handler" in obj
        assert "line" in obj


def test_json_method_is_uppercase():
    """JSON method field is uppercase."""
    result = run_routes(str(BOOKSHOP), "--format", "json")
    lines = [ln.strip() for ln in result.stdout.splitlines() if ln.strip()]
    for line in lines:
        obj = json.loads(line)
        assert obj["method"] == obj["method"].upper(), (
            f"method should be uppercase: {obj['method']}"
        )


def test_json_path_starts_with_slash():
    """JSON path field starts with / for resolved routes."""
    result = run_routes(str(BOOKSHOP), "--format", "json")
    lines = [ln.strip() for ln in result.stdout.splitlines() if ln.strip()]
    resolved = [json.loads(ln) for ln in lines if json.loads(ln).get("path", "").startswith("/")]
    assert len(resolved) > 0, "expected at least one resolved route path"


def test_json_line_is_positive_integer():
    """JSON line field is a positive integer."""
    result = run_routes(str(BOOKSHOP), "--format", "json")
    lines = [ln.strip() for ln in result.stdout.splitlines() if ln.strip()]
    for line in lines:
        obj = json.loads(line)
        assert isinstance(obj["line"], int) and obj["line"] > 0, (
            f"line should be a positive int: {obj['line']}"
        )
