"""Shell boundary checks for bounded runtime state and process identity."""

from __future__ import annotations

import os
import shutil
import signal
import subprocess
import sys
import time
from collections.abc import Iterator
from contextlib import contextmanager, suppress
from pathlib import Path

import pytest

ROOT = Path(__file__).resolve().parents[2]
RUNTIME_LIBRARY = ROOT / "deploy" / "container" / "runtime-lib.sh"


def run_shell(script: str, *arguments: str) -> subprocess.CompletedProcess[str]:
    return subprocess.run(
        ["bash", "-c", script, "runtime-test", str(RUNTIME_LIBRARY), *arguments],
        check=False,
        capture_output=True,
        text=True,
        timeout=5,
    )


def matches_command(pid: int | str, command: str) -> bool:
    """Ask runtime-lib.sh whether `pid` is running `command`.

    `pid` is deliberately not narrowed to int: the shell receives text either
    way, and the malformed-input cases below are about what it does with text
    that is not a pid at all.
    """
    return (
        run_shell(
            'source "$1"; omt_process_matches_command "$2" "$3"',
            str(pid),
            command,
        ).returncode
        == 0
    )


@contextmanager
def running(*argv: str) -> Iterator[subprocess.Popen[bytes]]:
    """Run `argv` until the block exits, once its /proc entry is readable.

    The identity checks read /proc/<pid>/exe and /proc/<pid>/cmdline, and a
    process that has been forked but has not reached its exec yet reports the
    parent's. Waiting for the expected argv removes that race without a sleep
    long enough to be a timeout in disguise.
    """
    # Own process group, so the teardown below reaps an interpreter's children
    # too rather than orphaning a long sleep for the rest of the run.
    process = subprocess.Popen(argv, start_new_session=True)
    try:
        deadline = time.monotonic() + 5
        expected = "\0".join(argv)
        while time.monotonic() < deadline:
            try:
                recorded = Path(f"/proc/{process.pid}/cmdline").read_text()
            except OSError:  # pragma: no cover - the entry appears with the pid
                recorded = ""
            if recorded.rstrip("\0") == expected:
                break
            time.sleep(0.01)
        else:  # pragma: no cover - only reached if the child never execs
            pytest.fail(f"{argv[0]} never reached its exec")
        yield process
    finally:
        with suppress(ProcessLookupError):
            os.killpg(process.pid, signal.SIGKILL)
        process.wait()


@pytest.fixture
def receiver_script(tmp_path: Path) -> Path:
    script = tmp_path / "omt-receiver.sh"
    # `sleep` as a child rather than `exec sleep`: the interpreter has to stay
    # on the pid for /proc/<pid>/exe to be the interpreter and the argv to be
    # the only thing identifying the script.
    script.write_text("#!/bin/bash\nsleep 30\n", encoding="utf-8")
    script.chmod(0o755)
    return script


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


def test_process_identity_matches_the_executable_behind_the_pid(tmp_path):
    """The strongest signal is /proc/<pid>/exe, which no argument can forge."""
    executable = tmp_path / "omt-receiver"
    # The interpreter rather than `sleep`: on Alpine, and anywhere busybox
    # provides the base utilities, `sleep` is a multicall binary that dispatches
    # on argv[0], so a copy named omt-receiver answers "unknown program" instead
    # of sleeping. CPython is a standalone executable and keeps working when
    # copied, which is what this needs -- a real ELF whose /proc/<pid>/exe is
    # this path.
    shutil.copy(sys.executable, executable)
    with running(str(executable), "-c", "import time; time.sleep(30)") as process:
        assert matches_command(process.pid, str(executable))
        assert not matches_command(process.pid, str(tmp_path / "other-receiver"))


def test_process_identity_matches_an_interpreted_receiver_by_argument(receiver_script):
    """A script's /proc/<pid>/exe is the interpreter, so the argv must answer.

    control-omt.sh guards every kill with this check, so a receiver launched
    through a shell wrapper has to remain recognisable or the controller would
    refuse to stop what it started.
    """
    with running("/bin/bash", str(receiver_script)) as process:
        assert matches_command(process.pid, str(receiver_script))


def test_process_identity_resolves_a_symlinked_argument(tmp_path, receiver_script):
    """An argv naming a link to the expected command still identifies it."""
    alias = tmp_path / "alias-receiver.sh"
    alias.symlink_to(receiver_script)
    with running("/bin/bash", str(alias)) as process:
        assert matches_command(process.pid, str(receiver_script))


def test_process_identity_rejects_an_unrelated_process(receiver_script):
    """A recycled pid running something else must not be taken for the receiver."""
    with running("/bin/sleep", "30") as process:
        assert not matches_command(process.pid, str(receiver_script))


@pytest.mark.parametrize(
    ("pid", "command"),
    [
        ("bad", "/bin/sleep"),
        ("0", "/bin/sleep"),
        ("-1", "/bin/sleep"),
        ("1", ""),
    ],
)
def test_process_identity_rejects_malformed_input(pid, command):
    assert not matches_command(pid, command)


def test_process_identity_rejects_a_pid_that_is_gone():
    process = subprocess.Popen(["/bin/sleep", "30"])
    process.kill()
    process.wait()
    assert not matches_command(process.pid, "/bin/sleep")
