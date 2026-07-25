"""Bounded subprocess execution."""

from __future__ import annotations

import os
import selectors
import signal
import subprocess
import time
from typing import IO

from ..models import CommandResult

COMMAND_OUTPUT_LIMIT = 256 * 1024
# Grace granted to a child that has already closed its pipes, and again after an
# escalation to SIGKILL. Both waits are bounded; see `_reap`.
REAP_GRACE_SECONDS = 1.0


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


def _kill_group(process: subprocess.Popen[bytes]) -> None:
    """SIGKILL the child's whole process group.

    `run_command` starts every child in its own session, so the child is its own
    group leader and signalling the group reaches whatever it spawned.
    `Popen.kill` signals only the direct child, which would leave a controller
    script's own children -- a receiver, an `openssl`, a `tcpdump` -- running
    with the pipes already abandoned. A group that has gone away between the
    drain loop and here is the expected outcome, not an error.
    """
    try:
        os.killpg(process.pid, signal.SIGKILL)
    except OSError:
        pass


def _reap(process: subprocess.Popen[bytes], timed_out: bool) -> None:
    """Settle the child without ever blocking indefinitely.

    Every wait here is bounded. `Popen.wait()` and `Popen.__exit__` without a
    timeout block forever on a process that is not dying, and a receiver wedged
    in an uninterruptible read against `/dev/dri` or ALSA is exactly that. That
    would convert a bounded, reported command timeout into a Gunicorn worker
    killed at `--timeout`, taking the operator's whole session with it.

    A child that outlives both waits is left for CPython's deferred `_active`
    reaping rather than waited on again.
    """
    if timed_out:
        _kill_group(process)
    try:
        process.wait(timeout=REAP_GRACE_SECONDS)
        return
    except subprocess.TimeoutExpired:
        pass
    # Both pipes reached EOF yet the child is still here. It has no way left to
    # report anything, so escalate rather than wait on it.
    _kill_group(process)
    try:
        process.wait(timeout=REAP_GRACE_SECONDS)
    except subprocess.TimeoutExpired:
        pass


def run_command(command: list[str], timeout: float) -> CommandResult:
    started = time.monotonic()
    try:
        process = subprocess.Popen(
            command,
            stdin=subprocess.DEVNULL,
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
            start_new_session=True,
        )
    except OSError as exc:
        return CommandResult(
            command=" ".join(command),
            duration_seconds=time.monotonic() - started,
            error=str(exc),
        )
    # Deliberately not `with Popen(...)`: its `__exit__` ends in an unbounded
    # `wait()`. The streams it would close are closed below instead.
    try:
        try:
            stdout, stderr, stdout_truncated, stderr_truncated, timed_out = _drain_bounded(
                process,
                timeout,
            )
        except OSError as exc:
            # The child is already spawned, so a failure to read its output has
            # to take the child down with it rather than leak it.
            _reap(process, timed_out=True)
            return CommandResult(
                command=" ".join(command),
                duration_seconds=time.monotonic() - started,
                error=str(exc),
            )
        _reap(process, timed_out)
    finally:
        for stream in filter(None, (process.stdout, process.stderr)):
            stream.close()
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
