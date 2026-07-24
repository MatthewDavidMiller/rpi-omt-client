from __future__ import annotations

import pytest

from omt_client.safe_io import ReadStatus, read_bytes, read_text
from omt_client.state_store import (
    SourceConfigurationError,
    SourceTarget,
    read_source_target,
    save_source_target,
)


def test_bounded_reads_distinguish_missing_unsafe_oversized_and_utf8(tmp_path):
    missing = read_bytes(tmp_path / "missing", 10)
    assert missing.status is ReadStatus.MISSING
    target = tmp_path / "target"
    target.write_bytes(b"value")
    assert read_bytes(target, 5).data == b"value"
    assert read_bytes(target, 4).status is ReadStatus.OVERSIZED
    link = tmp_path / "link"
    link.symlink_to(target)
    assert read_bytes(link, 10).status is ReadStatus.UNSAFE
    invalid = tmp_path / "invalid"
    invalid.write_bytes(b"\xff")
    assert read_text(invalid, 2).status is ReadStatus.INVALID_UTF8
    with pytest.raises(ValueError):
        read_bytes(target, -1)


@pytest.mark.parametrize(
    "target",
    [
        SourceTarget("discovered", "Studio Camera"),
        SourceTarget("direct", "omt://192.0.2.1:6400"),
    ],
)
def test_target_round_trip_is_private_and_atomic(tmp_path, target):
    path = tmp_path / "source_target.json"
    save_source_target(path, target)
    assert read_source_target(path) == target
    assert path.stat().st_mode & 0o777 == 0o600
    assert (tmp_path / "source_target.json.lock").stat().st_mode & 0o777 == 0o600
    save_source_target(path, None)
    assert read_source_target(path) is None


@pytest.mark.parametrize(
    "target",
    [
        SourceTarget("discovered", "bad\nname"),
        SourceTarget("direct", "host:6400"),
        SourceTarget("unknown", "value"),
    ],
)
def test_invalid_targets_are_rejected(tmp_path, target):
    with pytest.raises(SourceConfigurationError, match="invalid"):
        save_source_target(tmp_path / "source_target.json", target)


@pytest.mark.parametrize(
    "document",
    [
        b"not-json",
        b"{}",
        b'{"schema":2,"kind":"discovered","name":"Camera"}',
        b'{"schema":1,"kind":"discovered","name":"bad\\nname"}',
        b'{"schema":1,"kind":"direct","uri":"host:1"}',
        b'{"schema":1,"kind":"direct","uri":"omt://host:1","extra":true}',
    ],
)
def test_invalid_persisted_documents_fail_closed(tmp_path, document):
    path = tmp_path / "source_target.json"
    path.write_bytes(document)
    with pytest.raises(SourceConfigurationError):
        read_source_target(path)


def test_unsafe_directory_target_and_lock_fail_closed(tmp_path):
    real = tmp_path / "real"
    real.mkdir()
    linked = tmp_path / "linked"
    linked.symlink_to(real, target_is_directory=True)
    with pytest.raises(SourceConfigurationError, match="directory"):
        save_source_target(linked / "source_target.json", SourceTarget("discovered", "A"))

    target = real / "source_target.json"
    victim = real / "victim"
    victim.write_text("keep", encoding="utf-8")
    target.symlink_to(victim)
    with pytest.raises(SourceConfigurationError, match="path"):
        save_source_target(target, SourceTarget("discovered", "A"))
    assert victim.read_text(encoding="utf-8") == "keep"
    target.unlink()
    lock = real / "source_target.json.lock"
    lock.unlink()
    lock.symlink_to(victim)
    with pytest.raises(SourceConfigurationError, match="lock"):
        save_source_target(target, SourceTarget("discovered", "A"))


def test_source_target_oversized_and_nonregular_reads(tmp_path):
    path = tmp_path / "source_target.json"
    path.write_bytes(b"x" * 1025)
    with pytest.raises(SourceConfigurationError, match="unable to read"):
        read_source_target(path)
    path.unlink()
    path.mkdir()
    with pytest.raises(SourceConfigurationError, match="unable to read"):
        read_source_target(path)
