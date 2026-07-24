"""Failure-injection tests for shared bounded and durable file I/O."""

from __future__ import annotations

import errno
import os
from pathlib import Path

import pytest

from omt_client import safe_io


def test_write_all_retries_short_writes_and_rejects_no_progress(monkeypatch):
    writes: list[bytes] = []

    def short_write(_descriptor: int, value: memoryview) -> int:
        retained = bytes(value[:2])
        writes.append(retained)
        return len(retained)

    monkeypatch.setattr(os, "write", short_write)
    safe_io.write_all(10, b"abcdef")
    assert b"".join(writes) == b"abcdef"
    monkeypatch.setattr(os, "write", lambda *_args: 0)
    with pytest.raises(OSError, match="no progress"):
        safe_io.write_all(10, b"x")


@pytest.mark.parametrize("failure", ["write", "fsync", "replace"])
def test_atomic_replace_cleans_stage_after_every_precommit_failure(
    tmp_path: Path, monkeypatch, failure: str
):
    target = tmp_path / "value"
    target.write_bytes(b"old")
    if failure == "write":
        monkeypatch.setattr(
            safe_io,
            "write_all",
            lambda *_args: (_ for _ in ()).throw(OSError("write")),
        )
    elif failure == "fsync":
        original = os.fsync
        calls = 0

        def fail_first(descriptor: int):
            nonlocal calls
            calls += 1
            if calls == 1:
                raise OSError("fsync")
            return original(descriptor)

        monkeypatch.setattr(os, "fsync", fail_first)
    else:
        monkeypatch.setattr(os, "replace", lambda *_args: (_ for _ in ()).throw(OSError("replace")))

    with pytest.raises(OSError):
        safe_io.atomic_replace(target, b"new", 3)
    assert target.read_bytes() == b"old"
    assert not list(tmp_path.glob(".value.*"))


def test_atomic_replace_rejects_unsafe_parent_and_destination(tmp_path: Path):
    missing_parent = tmp_path / "missing" / "value"
    with pytest.raises(OSError, match="directory"):
        safe_io.atomic_replace(missing_parent, b"x", 1)
    directory_target = tmp_path / "directory"
    directory_target.mkdir()
    with pytest.raises(OSError, match="regular"):
        safe_io.atomic_replace(directory_target, b"x", 1)


def test_bounded_read_reports_open_read_and_postread_races(tmp_path: Path, monkeypatch):
    target = tmp_path / "value"
    target.write_bytes(b"value")
    original_fstat = os.fstat
    first = True

    def swapped_fstat(descriptor: int):
        nonlocal first
        result = original_fstat(descriptor)
        if first:
            first = False
            values = list(result)
            values[1] += 1
            return os.stat_result(values)
        return result

    monkeypatch.setattr(os, "fstat", swapped_fstat)
    assert safe_io.read_bytes(target, 10).status is safe_io.ReadStatus.UNSAFE
    monkeypatch.undo()

    monkeypatch.setattr(
        os,
        "open",
        lambda *_args, **_kwargs: (_ for _ in ()).throw(OSError(errno.ELOOP, "loop")),
    )
    assert safe_io.read_bytes(target, 10).status is safe_io.ReadStatus.UNSAFE
    monkeypatch.undo()

    original_lstat = os.lstat
    calls = 0

    def disappearing(path):
        nonlocal calls
        calls += 1
        if calls > 1:
            raise FileNotFoundError(errno.ENOENT, "missing", path)
        return original_lstat(path)

    monkeypatch.setattr(os, "lstat", disappearing)
    assert safe_io.read_bytes(target, 10).status is safe_io.ReadStatus.MISSING


def test_fixed_inode_write_preserves_identity_and_rejects_swaps(tmp_path: Path, monkeypatch):
    request = tmp_path / "request"
    request.touch(mode=0o600)
    os.chmod(request, 0o600)
    before = request.stat()
    safe_io.write_fixed_inode(request, b"request", 32)
    after = request.stat()
    assert request.read_bytes() == b"request"
    assert (before.st_dev, before.st_ino) == (after.st_dev, after.st_ino)
    with pytest.raises(OSError, match="exceeds"):
        safe_io.write_fixed_inode(request, b"too large", 2)
    os.chmod(request, 0o644)
    with pytest.raises(OSError, match="ownership or mode"):
        safe_io.write_fixed_inode(request, b"x", 2)

    os.chmod(request, 0o600)
    original_fstat = os.fstat

    def swapped(descriptor: int):
        result = original_fstat(descriptor)
        values = list(result)
        values[1] += 1
        return os.stat_result(values)

    monkeypatch.setattr(os, "fstat", swapped)
    with pytest.raises(OSError, match="changed while opening"):
        safe_io.write_fixed_inode(request, b"x", 2)
