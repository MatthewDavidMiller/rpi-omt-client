"""Bounded reads and atomic OMT target persistence."""

from __future__ import annotations

import errno
import fcntl
import json
import os
import secrets
import stat
from dataclasses import dataclass
from enum import StrEnum
from pathlib import Path

try:
    from .discovery import is_valid_direct_target, is_valid_source_name
except ImportError:
    from discovery import is_valid_direct_target, is_valid_source_name


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


class SourceConfigurationError(RuntimeError):
    """Raised when the single OMT target cannot be read or committed safely."""


@dataclass(frozen=True)
class SourceTarget:
    kind: str
    value: str

    @property
    def display_name(self) -> str:
        return self.value


SOURCE_TARGET_MAX_BYTES = 1024


def _identity(value: os.stat_result) -> tuple[int, int]:
    return value.st_dev, value.st_ino


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
    try:
        descriptor = os.open(path_text, flags)
        opened = os.fstat(descriptor)
        if not stat.S_ISREG(opened.st_mode) or _identity(opened) != _identity(before):
            return ReadResult(ReadStatus.UNSAFE, detail="file changed while opening")
        data = os.read(descriptor, maximum_bytes + 1)
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
        _identity(before) != _identity(after_descriptor)
        or _identity(after_descriptor) != _identity(after_path)
        or before.st_size != after_descriptor.st_size
        or after_descriptor.st_size != after_path.st_size
        or stat.S_ISLNK(after_path.st_mode)
    ):
        return ReadResult(ReadStatus.UNSAFE, detail="file changed while being read")
    return ReadResult(
        ReadStatus.OK,
        data=data,
        identity=_identity(after_descriptor),
        size=len(data),
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


def read_source_target(path: str | os.PathLike[str]) -> SourceTarget | None:
    result = read_bytes(path, SOURCE_TARGET_MAX_BYTES)
    if result.status is ReadStatus.MISSING:
        return None
    if not result.ok:
        raise SourceConfigurationError(
            f"unable to read saved OMT target: {result.detail or result.status.value}"
        )
    try:
        document = json.loads(result.data)
    except (UnicodeDecodeError, json.JSONDecodeError) as exc:
        raise SourceConfigurationError(f"saved OMT target is invalid JSON: {exc}") from exc
    if not isinstance(document, dict) or document.get("schema") != 1:
        raise SourceConfigurationError("saved OMT target has an invalid schema")
    if set(document) == {"schema", "kind", "name"} and document.get("kind") == "discovered":
        value = document.get("name")
        if isinstance(value, str) and is_valid_source_name(value):
            return SourceTarget("discovered", value)
    if set(document) == {"schema", "kind", "uri"} and document.get("kind") == "direct":
        value = document.get("uri")
        if isinstance(value, str) and is_valid_direct_target(value):
            return SourceTarget("direct", value)
    raise SourceConfigurationError("saved OMT target kind or value is invalid")


def _sync_directory(path: Path) -> None:
    descriptor = os.open(path, os.O_RDONLY | getattr(os, "O_DIRECTORY", 0))
    try:
        os.fsync(descriptor)
    finally:
        os.close(descriptor)


def save_source_target(
    path: str | os.PathLike[str],
    target: SourceTarget | None,
) -> None:
    target_path = Path(path).absolute()
    directory = target_path.parent
    if directory.is_symlink() or not directory.is_dir():
        raise SourceConfigurationError("OMT configuration directory is unsafe")
    lock_path = Path(f"{target_path}.lock")
    if lock_path.is_symlink():
        raise SourceConfigurationError("OMT target lock is unsafe")
    lock_flags = os.O_RDWR | os.O_CREAT
    lock_flags |= getattr(os, "O_CLOEXEC", 0) | getattr(os, "O_NOFOLLOW", 0)
    try:
        lock_descriptor = os.open(lock_path, lock_flags, 0o600)
    except OSError as exc:
        raise SourceConfigurationError(f"unable to lock saved OMT target: {exc}") from exc
    staged: Path | None = None
    try:
        if not stat.S_ISREG(os.fstat(lock_descriptor).st_mode):
            raise SourceConfigurationError("OMT target lock is not a regular file")
        os.fchmod(lock_descriptor, 0o600)
        fcntl.flock(lock_descriptor, fcntl.LOCK_EX)
        if target_path.is_symlink() or (
            target_path.exists() and not target_path.is_file()
        ):
            raise SourceConfigurationError("saved OMT target path is unsafe")
        if target is None:
            try:
                target_path.unlink()
            except FileNotFoundError:
                pass
            _sync_directory(directory)
            return
        if target.kind == "discovered" and is_valid_source_name(target.value):
            document = {"schema": 1, "kind": "discovered", "name": target.value}
        elif target.kind == "direct" and is_valid_direct_target(target.value):
            document = {"schema": 1, "kind": "direct", "uri": target.value}
        else:
            raise SourceConfigurationError("invalid OMT target kind or value")
        encoded = (
            json.dumps(document, ensure_ascii=False, separators=(",", ":")) + "\n"
        ).encode("utf-8")
        if len(encoded) > SOURCE_TARGET_MAX_BYTES:
            raise SourceConfigurationError("saved OMT target is oversized")
        staged = directory / f".source_target.{os.getpid()}.{secrets.token_hex(8)}"
        flags = os.O_WRONLY | os.O_CREAT | os.O_EXCL
        flags |= getattr(os, "O_CLOEXEC", 0) | getattr(os, "O_NOFOLLOW", 0)
        descriptor = os.open(staged, flags, 0o600)
        try:
            os.write(descriptor, encoded)
            os.fsync(descriptor)
        finally:
            os.close(descriptor)
        os.replace(staged, target_path)
        staged = None
        os.chmod(target_path, 0o600, follow_symlinks=False)
        _sync_directory(directory)
    except SourceConfigurationError:
        raise
    except OSError as exc:
        raise SourceConfigurationError(f"unable to save OMT target: {exc}") from exc
    finally:
        if staged is not None:
            try:
                staged.unlink()
            except FileNotFoundError:
                pass
        os.close(lock_descriptor)
