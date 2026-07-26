"""Shared bounded, race-aware file I/O primitives."""

from __future__ import annotations

import errno
import os
import secrets
import stat
from dataclasses import dataclass
from enum import StrEnum
from pathlib import Path


class ReadStatus(StrEnum):
    OK = "ok"
    MISSING = "missing"
    UNSAFE = "unsafe"
    OVERSIZED = "oversized"
    INVALID_UTF8 = "invalid_utf8"
    IO_ERROR = "io_error"


@dataclass(frozen=True)
class ReadResult:
    status: ReadStatus
    data: bytes = b""
    text: str = ""
    detail: str = ""
    identity: tuple[int, int] | None = None
    size: int | None = None

    @property
    def ok(self) -> bool:
        return self.status is ReadStatus.OK


def _identity(value: os.stat_result) -> tuple[int, int]:
    return value.st_dev, value.st_ino


def _snapshot(value: os.stat_result) -> tuple[int, int, int, int, int]:
    """Return the fields that change when file contents or identity change."""
    return (
        value.st_dev,
        value.st_ino,
        value.st_size,
        value.st_mtime_ns,
        value.st_ctime_ns,
    )


def _os_error(exc: OSError, action: str) -> ReadResult:
    if exc.errno == errno.ENOENT:
        return ReadResult(ReadStatus.MISSING, detail="file does not exist")
    if exc.errno in {errno.ELOOP, errno.ENOTDIR}:
        return ReadResult(ReadStatus.UNSAFE, detail=f"{action}: {exc}")
    return ReadResult(ReadStatus.IO_ERROR, detail=f"{action}: {exc}")


def read_bytes(path: str | os.PathLike[str], maximum_bytes: int) -> ReadResult:
    """Read one stable regular file without following its final symlink."""
    if maximum_bytes < 0:
        raise ValueError("maximum_bytes must not be negative")
    path_text = os.fspath(path)
    try:
        before = os.lstat(path_text)
    except OSError as exc:
        return _os_error(exc, "unable to inspect file")
    if stat.S_ISLNK(before.st_mode) or not stat.S_ISREG(before.st_mode):
        return ReadResult(
            ReadStatus.UNSAFE,
            detail="path is not a regular, non-symlinked file",
            identity=_identity(before),
            size=before.st_size,
        )
    if before.st_size > maximum_bytes:
        return ReadResult(
            ReadStatus.OVERSIZED,
            detail=f"file exceeds {maximum_bytes} bytes",
            identity=_identity(before),
            size=before.st_size,
        )

    flags = os.O_RDONLY | getattr(os, "O_CLOEXEC", 0) | getattr(os, "O_NOFOLLOW", 0)
    descriptor = -1
    data = bytearray()
    try:
        descriptor = os.open(path_text, flags)
        opened = os.fstat(descriptor)
        if not stat.S_ISREG(opened.st_mode) or _snapshot(opened) != _snapshot(before):
            return ReadResult(ReadStatus.UNSAFE, detail="file changed while opening")
        while len(data) <= maximum_bytes:
            chunk = os.read(descriptor, min(64 * 1024, maximum_bytes + 1 - len(data)))
            if not chunk:
                break
            data.extend(chunk)
        after_descriptor = os.fstat(descriptor)
    except OSError as exc:
        return _os_error(exc, "unable to read file")
    finally:
        if descriptor >= 0:
            os.close(descriptor)

    if len(data) > maximum_bytes or after_descriptor.st_size > maximum_bytes:
        return ReadResult(ReadStatus.OVERSIZED, detail=f"file exceeds {maximum_bytes} bytes")
    try:
        after_path = os.lstat(path_text)
    except OSError as exc:
        return _os_error(exc, "file changed after read")
    if (
        _snapshot(before) != _snapshot(after_descriptor)
        or _snapshot(after_descriptor) != _snapshot(after_path)
        or stat.S_ISLNK(after_path.st_mode)
    ):
        return ReadResult(ReadStatus.UNSAFE, detail="file changed while being read")
    value = bytes(data)
    return ReadResult(
        ReadStatus.OK,
        data=value,
        identity=_identity(after_descriptor),
        size=len(value),
    )


def read_text(path: str | os.PathLike[str], maximum_bytes: int) -> ReadResult:
    result = read_bytes(path, maximum_bytes)
    if not result.ok:
        return result
    try:
        text = result.data.decode("utf-8")
    except UnicodeDecodeError as exc:
        return ReadResult(
            ReadStatus.INVALID_UTF8,
            data=result.data,
            detail=f"file is not valid UTF-8: {exc}",
            identity=result.identity,
            size=result.size,
        )
    return ReadResult(
        ReadStatus.OK,
        data=result.data,
        text=text,
        identity=result.identity,
        size=result.size,
    )


def write_all(descriptor: int, value: bytes) -> None:
    view = memoryview(value)
    while view:
        written = os.write(descriptor, view)
        if written <= 0:
            raise OSError("write made no progress")
        view = view[written:]


def sync_directory(path: str | os.PathLike[str]) -> None:
    descriptor = os.open(path, os.O_RDONLY | getattr(os, "O_DIRECTORY", 0))
    try:
        os.fsync(descriptor)
    finally:
        os.close(descriptor)


def _regular_destination(path: Path) -> None:
    if path.exists() or path.is_symlink():
        target_stat = os.lstat(path)
        if stat.S_ISLNK(target_stat.st_mode) or not stat.S_ISREG(target_stat.st_mode):
            raise OSError("destination is not a regular file")


def atomic_replace(
    path: str | os.PathLike[str],
    value: bytes,
    maximum_bytes: int,
    *,
    mode: int = 0o600,
) -> None:
    """Durably replace a regular file and always remove an uncommitted stage."""
    if len(value) > maximum_bytes:
        raise OSError(f"content exceeds {maximum_bytes} bytes")
    destination = Path(path).absolute()
    parent = destination.parent
    if parent.is_symlink() or not parent.is_dir():
        raise OSError("destination directory is unsafe")
    _regular_destination(destination)
    temporary = parent / f".{destination.name}.{os.getpid()}.{secrets.token_hex(8)}"
    flags = (
        os.O_WRONLY
        | os.O_CREAT
        | os.O_EXCL
        | getattr(os, "O_CLOEXEC", 0)
        | getattr(os, "O_NOFOLLOW", 0)
    )
    descriptor = -1
    committed = False
    try:
        descriptor = os.open(temporary, flags, mode)
        os.fchmod(descriptor, mode)
        write_all(descriptor, value)
        os.fsync(descriptor)
        os.close(descriptor)
        descriptor = -1
        _regular_destination(destination)
        os.replace(temporary, destination)
        committed = True
        # The stage already has the exact requested mode from fchmod, and rename
        # preserves it. A chmod after replace is redundant and, if it fails,
        # reports the operation as failed after the new value is already live.
        sync_directory(parent)
    finally:
        if descriptor >= 0:
            os.close(descriptor)
        if not committed:
            try:
                temporary.unlink()
            except FileNotFoundError:
                pass


def write_fixed_inode(
    path: str | os.PathLike[str],
    value: bytes,
    maximum_bytes: int,
    *,
    required_mode: int = 0o600,
    required_uid: int | None = None,
) -> None:
    """Rewrite a pre-created request channel without replacing its inode."""
    if len(value) > maximum_bytes:
        raise OSError(f"request exceeds {maximum_bytes} bytes")
    path_text = os.fspath(path)
    before = os.lstat(path_text)
    expected_uid = os.geteuid() if required_uid is None else required_uid
    if (
        stat.S_ISLNK(before.st_mode)
        or not stat.S_ISREG(before.st_mode)
        or stat.S_IMODE(before.st_mode) != required_mode
        or before.st_uid != expected_uid
    ):
        raise OSError("request file has unsafe ownership or mode")
    flags = os.O_WRONLY | getattr(os, "O_CLOEXEC", 0) | getattr(os, "O_NOFOLLOW", 0)
    descriptor = os.open(path_text, flags)
    try:
        opened = os.fstat(descriptor)
        if not stat.S_ISREG(opened.st_mode) or _identity(opened) != _identity(before):
            raise OSError("request file changed while opening")
        os.ftruncate(descriptor, 0)
        write_all(descriptor, value)
        os.fsync(descriptor)
    finally:
        os.close(descriptor)
    after = os.lstat(path_text)
    if stat.S_ISLNK(after.st_mode) or _identity(after) != _identity(before):
        raise OSError("request file changed during write")
