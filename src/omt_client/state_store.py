"""Bounded reads and atomic OMT target persistence."""

from __future__ import annotations

import fcntl
import json
import os
import stat
import sys
from dataclasses import dataclass
from pathlib import Path

from .discovery import is_valid_direct_target, is_valid_source_name
from .safe_io import (
    ReadStatus,
    atomic_replace,
    read_bytes,
    sync_directory,
)


class SourceConfigurationError(RuntimeError):
    """Raised when the single OMT target cannot be read or committed safely."""


@dataclass(frozen=True)
class SourceTarget:
    kind: str
    value: str


SOURCE_TARGET_MAX_BYTES = 1024


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
    try:
        if not stat.S_ISREG(os.fstat(lock_descriptor).st_mode):
            raise SourceConfigurationError("OMT target lock is not a regular file")
        os.fchmod(lock_descriptor, 0o600)
        fcntl.flock(lock_descriptor, fcntl.LOCK_EX)
        if target_path.is_symlink() or (target_path.exists() and not target_path.is_file()):
            raise SourceConfigurationError("saved OMT target path is unsafe")
        if target is None:
            try:
                target_path.unlink()
            except FileNotFoundError:
                pass
            sync_directory(directory)
            return
        if target.kind == "discovered" and is_valid_source_name(target.value):
            document = {"schema": 1, "kind": "discovered", "name": target.value}
        elif target.kind == "direct" and is_valid_direct_target(target.value):
            document = {"schema": 1, "kind": "direct", "uri": target.value}
        else:
            raise SourceConfigurationError("invalid OMT target kind or value")
        encoded = (json.dumps(document, ensure_ascii=False, separators=(",", ":")) + "\n").encode(
            "utf-8"
        )
        if len(encoded) > SOURCE_TARGET_MAX_BYTES:
            raise SourceConfigurationError("saved OMT target is oversized")
        atomic_replace(target_path, encoded, SOURCE_TARGET_MAX_BYTES)
        sync_directory(directory)
    except SourceConfigurationError:
        raise
    except OSError as exc:
        raise SourceConfigurationError(f"unable to save OMT target: {exc}") from exc
    finally:
        os.close(lock_descriptor)


def play_target_value(path: str | os.PathLike[str]) -> str:
    """Return the receiver `--target` value for a saved configuration.

    Shared by the Flask services and `deploy/container/start-omt.sh` so the
    launcher cannot drift from the web-side schema and validation rules.
    """
    try:
        target = read_source_target(path)
    except SourceConfigurationError as exc:
        raise SystemExit(str(exc)) from exc
    if target is None:
        raise SystemExit("saved OMT target is missing")
    return target.value


def main(argv: list[str] | None = None) -> int:
    """Minimal CLI used by the container launcher."""
    arguments = sys.argv[1:] if argv is None else argv
    if len(arguments) != 2 or arguments[0] != "play-target":
        print("usage: python -m omt_client.state_store play-target PATH", file=sys.stderr)
        return 2
    print(play_target_value(arguments[1]))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
