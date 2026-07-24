"""Bounded subprocess execution."""

from __future__ import annotations

import subprocess
import time

from ..models import CommandResult

COMMAND_OUTPUT_LIMIT = 256 * 1024


def run_command(command: list[str], timeout: float) -> CommandResult:
    started = time.monotonic()
    try:
        completed = subprocess.run(
            command,
            stdin=subprocess.DEVNULL,
            capture_output=True,
            timeout=timeout,
            check=False,
            start_new_session=True,
        )
        stdout = completed.stdout[:COMMAND_OUTPUT_LIMIT].decode("utf-8", "replace")
        stderr = completed.stderr[:COMMAND_OUTPUT_LIMIT].decode("utf-8", "replace")
        return CommandResult(
            command=" ".join(command),
            returncode=completed.returncode,
            stdout=stdout,
            stderr=stderr,
            duration_seconds=time.monotonic() - started,
            stdout_truncated=len(completed.stdout) > COMMAND_OUTPUT_LIMIT,
            stderr_truncated=len(completed.stderr) > COMMAND_OUTPUT_LIMIT,
        )
    except subprocess.TimeoutExpired as exc:
        stdout = (exc.stdout or b"")[:COMMAND_OUTPUT_LIMIT].decode("utf-8", "replace")
        stderr = (exc.stderr or b"")[:COMMAND_OUTPUT_LIMIT].decode("utf-8", "replace")
        return CommandResult(
            command=" ".join(command),
            stdout=stdout,
            stderr=stderr,
            duration_seconds=time.monotonic() - started,
            timed_out=True,
            error=f"Command exceeded {timeout:g} seconds.",
        )
    except OSError as exc:
        return CommandResult(
            command=" ".join(command),
            duration_seconds=time.monotonic() - started,
            error=str(exc),
        )
