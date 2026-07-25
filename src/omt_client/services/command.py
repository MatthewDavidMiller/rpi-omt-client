"""Bounded subprocess execution."""

from __future__ import annotations

import os
import selectors
import subprocess
import time
from typing import IO

from ..models import CommandResult

COMMAND_OUTPUT_LIMIT = 256 * 1024


def _drain_bounded(
    process: subprocess.Popen[bytes],
    timeout: float,
) -> tuple[bytes, bytes, bool, bool, bool]:
    """Read stdout/stderr up to the shared limit without buffering the rest.

    Once a stream hits the limit its pipe stays open but unread bytes are
    discarded in place so a noisy child cannot grow this worker's RSS. The
    process is killed when the wall-clock budget expires.
    """
    deadline = time.monotonic() + timeout
    streams: dict[int, bytearray] = {}
    truncated: dict[int, bool] = {}
    handles: dict[int, IO[bytes]] = {}
    selector = selectors.DefaultSelector()
    for handle in (process.stdout, process.stderr):
        if handle is None:
            continue
        fd = handle.fileno()
        streams[fd] = bytearray()
        truncated[fd] = False
        handles[fd] = handle
        selector.register(handle, selectors.EVENT_READ)
    timed_out = False
    try:
        while selector.get_map():
            remaining = deadline - time.monotonic()
            if remaining <= 0:
                timed_out = True
                break
            for key, _events in selector.select(remaining):
                fd = key.fd
                chunk = os.read(fd, 65536)
                if not chunk:
                    selector.unregister(handles[fd])
                    continue
                if truncated[fd]:
                    continue
                buffer = streams[fd]
                room = COMMAND_OUTPUT_LIMIT - len(buffer)
                if room <= 0:
                    truncated[fd] = True
                    continue
                if len(chunk) > room:
                    buffer.extend(chunk[:room])
                    truncated[fd] = True
                else:
                    buffer.extend(chunk)
        if timed_out:
            process.kill()
        try:
            process.wait(timeout=1)
        except subprocess.TimeoutExpired:
            pass
    finally:
        selector.close()
    stdout_fd = process.stdout.fileno() if process.stdout is not None else None
    stderr_fd = process.stderr.fileno() if process.stderr is not None else None
    stdout = bytes(streams[stdout_fd]) if stdout_fd is not None else b""
    stderr = bytes(streams[stderr_fd]) if stderr_fd is not None else b""
    return (
        stdout,
        stderr,
        truncated.get(stdout_fd, False) if stdout_fd is not None else False,
        truncated.get(stderr_fd, False) if stderr_fd is not None else False,
        timed_out,
    )


def run_command(command: list[str], timeout: float) -> CommandResult:
    started = time.monotonic()
    try:
        with subprocess.Popen(
            command,
            stdin=subprocess.DEVNULL,
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
            start_new_session=True,
        ) as process:
            stdout, stderr, stdout_truncated, stderr_truncated, timed_out = _drain_bounded(
                process,
                timeout,
            )
            if timed_out:
                return CommandResult(
                    command=" ".join(command),
                    stdout=stdout.decode("utf-8", "replace"),
                    stderr=stderr.decode("utf-8", "replace"),
                    duration_seconds=time.monotonic() - started,
                    timed_out=True,
                    error=f"Command exceeded {timeout:g} seconds.",
                    stdout_truncated=stdout_truncated,
                    stderr_truncated=stderr_truncated,
                )
            return CommandResult(
                command=" ".join(command),
                returncode=process.returncode,
                stdout=stdout.decode("utf-8", "replace"),
                stderr=stderr.decode("utf-8", "replace"),
                duration_seconds=time.monotonic() - started,
                stdout_truncated=stdout_truncated,
                stderr_truncated=stderr_truncated,
            )
    except OSError as exc:
        return CommandResult(
            command=" ".join(command),
            duration_seconds=time.monotonic() - started,
            error=str(exc),
        )
