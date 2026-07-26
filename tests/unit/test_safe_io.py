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


def test_atomic_replace_does_not_add_a_fallible_chmod_after_commit(tmp_path: Path, monkeypatch):
    """The private stage already has its final mode. A second chmod after rename
    can only turn a committed update into a reported failure."""
    target = tmp_path / "value"
    target.write_bytes(b"old")
    monkeypatch.setattr(
        os,
        "chmod",
        lambda *_args, **_kwargs: (_ for _ in ()).throw(OSError("chmod failed")),
    )

    safe_io.atomic_replace(target, b"new", 3)

    assert target.read_bytes() == b"new"
    assert target.stat().st_mode & 0o777 == 0o600


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


def test_unexpected_os_errors_are_reported_as_io_errors(tmp_path: Path):
    """ELOOP/ENOTDIR mean "unsafe" and ENOENT means "missing", but anything else
    -- a permission or type error -- must still fail closed rather than raise."""
    directory = tmp_path / "directory"
    directory.mkdir()
    unreadable = tmp_path / "unreadable"
    unreadable.write_bytes(b"secret")
    os.chmod(unreadable, 0o000)
    try:
        result = safe_io.read_bytes(unreadable, 10)
        if os.geteuid() == 0:
            pytest.skip("root bypasses the permission check")
        assert result.status is safe_io.ReadStatus.IO_ERROR
        assert "unable to read file" in result.detail
    finally:
        os.chmod(unreadable, 0o600)

    nested = safe_io.read_bytes(unreadable / "child", 10)
    assert nested.status is safe_io.ReadStatus.UNSAFE


def test_bounded_read_rejects_a_file_that_grows_past_the_limit(tmp_path: Path, monkeypatch):
    """lstat reports a size within the limit, then the file grows before the
    read. Trusting the first stat would return a partial record as if it were
    whole."""
    target = tmp_path / "value"
    target.write_bytes(b"12345")
    original_read = os.read
    grown = False

    def growing_read(descriptor: int, count: int) -> bytes:
        nonlocal grown
        if not grown:
            grown = True
            with open(target, "ab") as handle:
                handle.write(b"6789")
        return original_read(descriptor, count)

    monkeypatch.setattr(os, "read", growing_read)
    assert safe_io.read_bytes(target, 5).status is safe_io.ReadStatus.OVERSIZED


def test_bounded_read_rejects_a_file_replaced_during_the_read(tmp_path: Path, monkeypatch):
    """The descriptor stays valid across a rename, so only comparing the final
    lstat identity catches a swap committed mid-read."""
    target = tmp_path / "value"
    target.write_bytes(b"12345")
    replacement = tmp_path / "replacement"
    replacement.write_bytes(b"abcde")
    original_read = os.read
    swapped = False

    def swapping_read(descriptor: int, count: int) -> bytes:
        nonlocal swapped
        if not swapped:
            swapped = True
            os.replace(replacement, target)
        return original_read(descriptor, count)

    monkeypatch.setattr(os, "read", swapping_read)
    result = safe_io.read_bytes(target, 16)
    assert result.status is safe_io.ReadStatus.UNSAFE
    assert "changed while being read" in result.detail


def test_bounded_read_rejects_same_size_in_place_mutation(tmp_path: Path, monkeypatch):
    """Identity and size alone do not prove stability when a writer truncates
    and rewrites the same inode with an equally sized record."""
    target = tmp_path / "value"
    target.write_bytes(b"12345")
    original_read = os.read
    mutated = False

    def mutating_read(descriptor: int, count: int) -> bytes:
        nonlocal mutated
        if not mutated:
            mutated = True
            target.write_bytes(b"abcde")
        return original_read(descriptor, count)

    monkeypatch.setattr(os, "read", mutating_read)
    result = safe_io.read_bytes(target, 16)
    assert result.status is safe_io.ReadStatus.UNSAFE
    assert "changed while being read" in result.detail


def test_atomic_replace_tolerates_a_stage_removed_by_someone_else(tmp_path: Path, monkeypatch):
    """The rollback must not mask the original failure with a spurious
    FileNotFoundError when the stage is already gone."""
    target = tmp_path / "value"
    target.write_bytes(b"old")

    def unlink_then_fail(source, destination):
        os.unlink(source)
        raise OSError("replace refused")

    monkeypatch.setattr(os, "replace", unlink_then_fail)
    with pytest.raises(OSError, match="replace refused"):
        safe_io.atomic_replace(target, b"new", 3)
    assert target.read_bytes() == b"old"
    assert not list(tmp_path.glob(".value.*"))


def test_fixed_inode_write_rejects_a_swap_committed_after_the_write(tmp_path: Path, monkeypatch):
    """The pre-open and post-open checks both pass, then the request file is
    replaced before the final lstat. Without the closing check the caller would
    believe its record reached the channel the host reads."""
    request = tmp_path / "request"
    request.touch(mode=0o600)
    os.chmod(request, 0o600)
    replacement = tmp_path / "replacement"
    replacement.touch(mode=0o600)
    original_fsync = os.fsync

    def swap_after_fsync(descriptor: int) -> None:
        original_fsync(descriptor)
        os.replace(replacement, request)

    monkeypatch.setattr(os, "fsync", swap_after_fsync)
    with pytest.raises(OSError, match="changed during write"):
        safe_io.write_fixed_inode(request, b"record", 32)


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
