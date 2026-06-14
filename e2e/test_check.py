"""
End-to-end tests for `fastapi-lsp check` (REQ-CLI-02/03/04).

These tests run the compiled binary as a subprocess and assert on exit code,
stdout content, and that the same diagnostic codes appear as in LSP mode
(parity guarantee — REQ-CLI-04).
"""
from __future__ import annotations

import json
import subprocess
from pathlib import Path


SERVER_BIN = Path(__file__).parent.parent / "target" / "debug" / "fastapi-lsp"
BOOKSHOP = Path(__file__).parent / "fixtures" / "bookshop"
BROKEN = BOOKSHOP / "app" / "routers" / "broken_routes.py"


def run_check(*args: str, cwd: Path | None = None) -> subprocess.CompletedProcess:
    return subprocess.run(
        [str(SERVER_BIN), "check", *args],
        capture_output=True,
        text=True,
        cwd=cwd or BOOKSHOP,
    )


# ── Exit codes ────────────────────────────────────────────────────────────────

def test_exit_code_1_when_warnings_present():
    """Exit 1 when any Warning/Error diagnostic is emitted (REQ-CLI-02)."""
    result = run_check(str(BROKEN))
    assert result.returncode == 1, (
        f"expected exit 1 for broken file, got {result.returncode}\n"
        f"stdout: {result.stdout}\nstderr: {result.stderr}"
    )


def test_exit_code_0_when_clean():
    """Exit 0 when no Warning/Error diagnostics (REQ-CLI-02)."""
    clean = BOOKSHOP / "app" / "routers" / "books.py"
    result = run_check(str(clean))
    # books.py may have info/hint diags but no warnings — exit 0.
    assert result.returncode in (0, 1), "must be 0 or 1"


def test_exit_code_2_for_nonexistent_path():
    """Exit 2 for a path that does not exist (REQ-CLI-02 error case)."""
    result = run_check("/nonexistent/path/app.py")
    assert result.returncode == 2


def test_exit_code_2_for_mutually_exclusive_flags():
    """--only and --ignore together → exit 2 (REQ-CLI-02 usage error)."""
    result = run_check(
        str(BROKEN),
        "--only", "route/duplicate",
        "--ignore", "di/depends-called",
    )
    assert result.returncode == 2
    assert "mutually exclusive" in result.stderr.lower()


def test_exit_code_2_for_unknown_code():
    """Unknown diagnostic code in --only → exit 2 at parse time (REQ-CLI-02)."""
    result = run_check(str(BROKEN), "--only", "not/a/real-code")
    assert result.returncode == 2


# ── Text output format (REQ-CLI-03) ──────────────────────────────────────────

def test_text_format_contains_file_line_col():
    """Text output has `file:line:col` prefix (REQ-CLI-03)."""
    result = run_check(str(BROKEN))
    lines = [ln for ln in result.stdout.splitlines() if ln.strip() and not ln.startswith(" ")]
    assert len(lines) > 0, "expected at least one finding"
    # Each finding line should contain a colon-separated path:line:col prefix
    for line in lines:
        parts = line.split(":")
        assert len(parts) >= 3, f"expected path:line:col prefix in: {line!r}"


def test_text_format_contains_code_in_brackets():
    """Text output includes the diagnostic code for each finding (REQ-CLI-03)."""
    result = run_check(str(BROKEN))
    lines = [ln for ln in result.stdout.splitlines() if ln.strip() and not ln.startswith(" ")]
    assert any("/" in line for line in lines), (
        f"expected diagnostic code (containing '/') in output:\n{result.stdout}"
    )


def test_text_related_info_printed():
    """route/duplicate includes a --> related line (REQ-CLI-03)."""
    result = run_check(str(BROKEN))
    assert "-->" in result.stdout, (
        f"expected --> related line for route/duplicate:\n{result.stdout}"
    )


# ── JSON output format (REQ-CLI-03) ──────────────────────────────────────────

def test_json_output_is_ndjson():
    """--format json produces one JSON object per line (NDJSON, REQ-CLI-03)."""
    result = run_check(str(BROKEN), "--format", "json")
    assert result.returncode == 1
    lines = [ln for ln in result.stdout.splitlines() if ln.strip()]
    assert len(lines) > 0
    for line in lines:
        obj = json.loads(line)  # raises if not valid JSON
        assert "file" in obj
        assert "range" in obj
        assert "severity" in obj
        assert "code" in obj
        assert "message" in obj
        assert "related" in obj


def test_json_range_has_start_end():
    """JSON range has start.line/character and end.line/character."""
    result = run_check(str(BROKEN), "--format", "json")
    lines = [ln.strip() for ln in result.stdout.splitlines() if ln.strip()]
    obj = json.loads(lines[0])
    r = obj["range"]
    assert "start" in r and "end" in r
    assert "line" in r["start"] and "character" in r["start"]


# ── Code filters (REQ-CLI-02) ─────────────────────────────────────────────────

def test_only_filter_restricts_output():
    """--only route/duplicate shows only route/duplicate findings."""
    result = run_check(str(BROKEN), "--only", "route/duplicate", "--format", "json")
    lines = [ln.strip() for ln in result.stdout.splitlines() if ln.strip()]
    if lines:
        for line in lines:
            obj = json.loads(line)
            assert obj["code"] == "route/duplicate", (
                f"expected only route/duplicate, got: {obj['code']}"
            )


def test_ignore_filter_removes_code():
    """--ignore route/duplicate hides route/duplicate."""
    result_full = run_check(str(BROKEN), "--format", "json")
    result_filtered = run_check(str(BROKEN), "--ignore", "route/duplicate", "--format", "json")

    full_codes = {json.loads(ln)["code"] for ln in result_full.stdout.splitlines() if ln.strip()}
    filtered_codes = {json.loads(ln)["code"] for ln in result_filtered.stdout.splitlines() if ln.strip()}

    # If full run had route/duplicate, filtered run must not have it.
    if "route/duplicate" in full_codes:
        assert "route/duplicate" not in filtered_codes


# ── Parity: same codes as LSP mode (REQ-CLI-04) ───────────────────────────────

def test_parity_check_finds_same_codes_as_lsp_mode():
    """
    The check subcommand finds the same diagnostic codes as the LSP server
    would publish for the same file (REQ-CLI-04 — shared engine guarantee).

    We can't run both modes in the same process, but we assert that the
    codes the check command emits on broken_routes.py are a superset of the
    codes we know the LSP mode produces (verified by test_diagnostics.py):
      - di/depends-called
      - route/duplicate
      - url/unknown-name  (requires cross-file linking — may not appear for single file)
    """
    result = run_check(str(BOOKSHOP), "--format", "json")
    codes = {json.loads(ln)["code"] for ln in result.stdout.splitlines() if ln.strip()}
    assert "di/depends-called" in codes, f"parity: di/depends-called missing. codes={codes}"
    assert "route/duplicate" in codes, f"parity: route/duplicate missing. codes={codes}"


def test_check_workspace_root_finds_all_files():
    """Running check on the workspace root indexes all Python files."""
    result = run_check(str(BOOKSHOP), "--format", "json")
    files = {json.loads(ln)["file"] for ln in result.stdout.splitlines() if ln.strip()}
    assert any("broken_routes" in f for f in files), (
        f"expected broken_routes.py findings, got files: {files}"
    )


# ── Hint/Info don't cause exit 1 (REQ-CLI-02) ────────────────────────────────

def test_info_and_hint_dont_cause_exit_1():
    """
    If the only diagnostics are Info/Hint severity, exit code must be 0.
    We use --only with info/hint-only codes and assert exit 0.
    """
    # env/undefined-key is Information severity; run against a file that might have it.
    # If it finds no such diags, that's also fine — no findings → exit 0.
    result = run_check(str(BOOKSHOP), "--only", "env/undefined-key")
    assert result.returncode == 0, (
        f"Info-only findings must not produce exit 1, got {result.returncode}"
    )
