"""Bounded reads and atomic OMT target persistence."""

from __future__ import annotations

import fcntl
import json
import os
import re
import stat
import sys
from collections.abc import Callable
from dataclasses import dataclass
from pathlib import Path

from .discovery import is_valid_direct_target, is_valid_source_name
from .json_document import JsonDocumentError, load_json_document
from .safe_io import (
    ReadStatus,
    atomic_replace,
    read_bytes,
    sync_directory,
)


class SourceConfigurationError(RuntimeError):
    """Raised when the single OMT target cannot be read or committed safely."""


class VideoCeilingError(RuntimeError):
    """Raised when the decode ceiling cannot be read, parsed, or committed."""


@dataclass(frozen=True)
class SourceTarget:
    kind: str
    value: str


SOURCE_TARGET_MAX_BYTES = 1024
VIDEO_CEILING_MAX_BYTES = 256

# Absolute limits, mirroring omt_protocol.parse_video_header and
# omt_receiver_core::VideoCeiling. Neither a board profile nor an operator
# override may exceed them: they are what size the decoder's allocations.
CEILING_MAX_WIDTH = 1920
CEILING_MAX_HEIGHT = 1080
CEILING_MAX_FPS = 60
CEILING_MIN_DIMENSION = 16
CEILING_MAX_SHAPES = 4

_CEILING_SHAPE = re.compile(r"^([1-9][0-9]{1,3})x([1-9][0-9]{1,3})@([1-9][0-9]{0,2})$")


def parse_video_ceiling(value: str) -> str:
    """Return a normalized ceiling string, or raise `VideoCeilingError`.

    The same grammar as `deploy/lib/board-profile.sh` and the receiver's
    `--video-ceiling`. This is the operator-facing end of it, so every
    rejection carries the reason rather than falling back to a default: quietly
    substituting one would present the appliance as capable of something nobody
    chose.
    """
    shapes = value.split(",")
    if not 1 <= len(shapes) <= CEILING_MAX_SHAPES:
        raise VideoCeilingError(
            f"A video limit must list between 1 and {CEILING_MAX_SHAPES} resolutions."
        )
    for shape in shapes:
        matched = _CEILING_SHAPE.match(shape)
        if matched is None:
            raise VideoCeilingError(
                f"Invalid video limit: {shape or '(empty)'}. Expected WIDTHxHEIGHT@FPS."
            )
        width, height, fps = (int(group) for group in matched.groups())
        if not CEILING_MIN_DIMENSION <= width <= CEILING_MAX_WIDTH:
            raise VideoCeilingError(
                f"Width {width} is outside {CEILING_MIN_DIMENSION}-{CEILING_MAX_WIDTH}."
            )
        if not CEILING_MIN_DIMENSION <= height <= CEILING_MAX_HEIGHT:
            raise VideoCeilingError(
                f"Height {height} is outside {CEILING_MIN_DIMENSION}-{CEILING_MAX_HEIGHT}."
            )
        if not 1 <= fps <= CEILING_MAX_FPS:
            raise VideoCeilingError(f"Frame rate {fps} is outside 1-{CEILING_MAX_FPS}.")
    return ",".join(shapes)


def describe_video_ceiling(value: str) -> str:
    """Render a ceiling as the prose the dashboard and receiver both show."""
    shapes = []
    for shape in value.split(","):
        matched = _CEILING_SHAPE.match(shape)
        if matched is None:
            return value
        width, height, fps = matched.groups()
        shapes.append(f"{width}x{height} at {int(fps)} fps")
    return ", or ".join(shapes)


def read_video_ceiling(path: str | os.PathLike[str]) -> str | None:
    """Return the operator's ceiling override, or None when unset."""
    result = read_bytes(path, VIDEO_CEILING_MAX_BYTES)
    if result.status is ReadStatus.MISSING:
        return None
    if not result.ok:
        raise VideoCeilingError(
            f"unable to read the saved video limit: {result.detail or result.status.value}"
        )
    try:
        document = load_json_document(result.data)
    except JsonDocumentError as exc:
        raise VideoCeilingError(f"saved video limit is invalid JSON: {exc}") from exc
    if (
        not isinstance(document, dict)
        or set(document) != {"schema", "ceiling"}
        or type(document.get("schema")) is not int
        or document.get("schema") != 1
    ):
        raise VideoCeilingError("saved video limit has an invalid schema")
    ceiling = document.get("ceiling")
    if not isinstance(ceiling, str):
        raise VideoCeilingError("saved video limit is not a string")
    return parse_video_ceiling(ceiling)


def save_video_ceiling(path: str | os.PathLike[str], ceiling: str | None) -> None:
    """Commit or clear the override, under the source record's safety rules."""

    def build() -> dict[str, object] | None:
        return None if ceiling is None else {"schema": 1, "ceiling": parse_video_ceiling(ceiling)}

    _commit_record(
        path,
        build,
        VIDEO_CEILING_MAX_BYTES,
        VideoCeilingError,
        "video limit",
    )


def effective_video_ceiling(path: str | os.PathLike[str], board_default: str) -> str:
    """Return the override if one is saved and valid, else the board default.

    The board default is validated too. It arrives from the installer through
    the container environment, and starting playback with an unparseable limit
    would fail in the receiver's argument parsing with far less context.
    """
    default = parse_video_ceiling(board_default)
    override = read_video_ceiling(path)
    return override if override is not None else default


def read_source_target(path: str | os.PathLike[str]) -> SourceTarget | None:
    result = read_bytes(path, SOURCE_TARGET_MAX_BYTES)
    if result.status is ReadStatus.MISSING:
        return None
    if not result.ok:
        raise SourceConfigurationError(
            f"unable to read saved OMT target: {result.detail or result.status.value}"
        )
    try:
        document = load_json_document(result.data)
    except JsonDocumentError as exc:
        raise SourceConfigurationError(f"saved OMT target is invalid JSON: {exc}") from exc
    if (
        not isinstance(document, dict)
        or type(document.get("schema")) is not int
        or document.get("schema") != 1
    ):
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


def _commit_record(
    path: str | os.PathLike[str],
    build: Callable[[], dict[str, object] | None],
    maximum_bytes: int,
    failure: type[SourceConfigurationError] | type[VideoCeilingError],
    noun: str,
) -> None:
    """Commit or remove one small JSON record on the config volume.

    Both persisted records -- the OMT target and the video limit -- live on the
    same operator-writable volume and need the same discipline: an exclusive
    lock so two workers cannot interleave, a refusal to follow a symlink at the
    directory, the lock, or the record itself, a size bound, and an atomic
    replace. `build` runs under the lock and returns the document, or `None` to
    remove the record. `noun` names the record in every message.
    """
    target_path = Path(path).absolute()
    directory = target_path.parent
    if directory.is_symlink() or not directory.is_dir():
        raise failure("OMT configuration directory is unsafe")
    lock_path = Path(f"{target_path}.lock")
    if lock_path.is_symlink():
        raise failure(f"{noun} lock is unsafe")
    lock_flags = os.O_RDWR | os.O_CREAT
    lock_flags |= getattr(os, "O_CLOEXEC", 0) | getattr(os, "O_NOFOLLOW", 0)
    try:
        lock_descriptor = os.open(lock_path, lock_flags, 0o600)
    except OSError as exc:
        raise failure(f"unable to lock saved {noun}: {exc}") from exc
    try:
        if not stat.S_ISREG(os.fstat(lock_descriptor).st_mode):
            raise failure(f"{noun} lock is not a regular file")
        os.fchmod(lock_descriptor, 0o600)
        fcntl.flock(lock_descriptor, fcntl.LOCK_EX)
        if target_path.is_symlink() or (target_path.exists() and not target_path.is_file()):
            raise failure(f"saved {noun} path is unsafe")
        document = build()
        if document is None:
            try:
                target_path.unlink()
            except FileNotFoundError:
                pass
            sync_directory(directory)
            return
        encoded = (json.dumps(document, ensure_ascii=False, separators=(",", ":")) + "\n").encode(
            "utf-8"
        )
        if len(encoded) > maximum_bytes:
            raise failure(f"saved {noun} is oversized")
        atomic_replace(target_path, encoded, maximum_bytes)
    except (SourceConfigurationError, VideoCeilingError):
        raise
    except OSError as exc:
        raise failure(f"unable to save {noun}: {exc}") from exc
    finally:
        os.close(lock_descriptor)


def save_source_target(
    path: str | os.PathLike[str],
    target: SourceTarget | None,
) -> None:
    def build() -> dict[str, object] | None:
        if target is None:
            return None
        if target.kind == "discovered" and is_valid_source_name(target.value):
            return {"schema": 1, "kind": "discovered", "name": target.value}
        if target.kind == "direct" and is_valid_direct_target(target.value):
            return {"schema": 1, "kind": "direct", "uri": target.value}
        raise SourceConfigurationError("invalid OMT target kind or value")

    _commit_record(
        path,
        build,
        SOURCE_TARGET_MAX_BYTES,
        SourceConfigurationError,
        "OMT target",
    )


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


def effective_ceiling_value(path: str | os.PathLike[str], board_default: str) -> str:
    """Return the receiver `--video-ceiling` value for the saved configuration.

    Shared by the Flask services and `deploy/container/start-omt.sh` so the
    launcher cannot drift from the web-side schema and validation rules.
    """
    try:
        return effective_video_ceiling(path, board_default)
    except VideoCeilingError as exc:
        raise SystemExit(str(exc)) from exc


def main(argv: list[str] | None = None) -> int:
    """Minimal CLI used by the container launcher."""
    arguments = sys.argv[1:] if argv is None else argv
    if len(arguments) == 2 and arguments[0] == "play-target":
        print(play_target_value(arguments[1]))
        return 0
    if len(arguments) == 3 and arguments[0] == "video-ceiling":
        print(effective_ceiling_value(arguments[1], arguments[2]))
        return 0
    print(
        "usage: python -m omt_client.state_store play-target PATH\n"
        "       python -m omt_client.state_store video-ceiling PATH BOARD_DEFAULT",
        file=sys.stderr,
    )
    return 2


if __name__ == "__main__":
    raise SystemExit(main())
