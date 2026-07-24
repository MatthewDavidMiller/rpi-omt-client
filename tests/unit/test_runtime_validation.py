"""Shell boundary checks for bounded runtime state."""

from __future__ import annotations

import subprocess
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
RUNTIME_LIBRARY = ROOT / "deploy" / "container" / "runtime-lib.sh"


def run_shell(script: str, *arguments: str) -> subprocess.CompletedProcess[str]:
    return subprocess.run(
        ["bash", "-c", script, "runtime-test", str(RUNTIME_LIBRARY), *arguments],
        check=False,
        capture_output=True,
        text=True,
        timeout=2,
    )


def test_bounded_state_reader_accepts_stable_regular_file(tmp_path):
    state = tmp_path / "state"
    state.write_text("value", encoding="utf-8")
    result = run_shell(
        'source "$1"; omt_read_bounded_state "$2" 5; printf "%s" "$OMT_BOUNDED_STATE_VALUE"',
        str(state),
    )
    assert result.returncode == 0
    assert result.stdout == "value"


def test_bounded_state_reader_rejects_oversized_symlink_and_bad_limit(tmp_path):
    target = tmp_path / "target"
    target.write_text("oversized", encoding="utf-8")
    link = tmp_path / "link"
    link.symlink_to(target)
    for path, limit in ((target, "2"), (link, "20"), (target, "bad")):
        result = run_shell(
            'source "$1"; omt_read_bounded_state "$2" "$3"',
            str(path),
            limit,
        )
        assert result.returncode != 0


def test_process_start_time_rejects_invalid_pid_and_reads_current_shell():
    result = run_shell(
        'source "$1"; omt_proc_start_time "$$"',
    )
    assert result.returncode == 0
    assert result.stdout.strip().isdigit()
    assert run_shell('source "$1"; omt_proc_start_time bad').returncode != 0
