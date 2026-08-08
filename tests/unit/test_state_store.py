from __future__ import annotations

import os

import pytest
from conftest import raises

from omt_client.safe_io import ReadStatus, read_bytes, read_text
from omt_client.state_store import (
    SourceConfigurationError,
    SourceTarget,
    VideoCeilingError,
    describe_video_ceiling,
    effective_video_ceiling,
    parse_video_ceiling,
    play_target_value,
    read_source_target,
    read_video_ceiling,
    save_source_target,
    save_video_ceiling,
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
        b'{"schema":true,"kind":"discovered","name":"Camera"}',
        b'{"schema":1.0,"kind":"discovered","name":"Camera"}',
        b'{"schema":NaN,"kind":"discovered","name":"Camera"}',
        b'{"schema":2,"schema":1,"kind":"discovered","name":"Camera"}',
        '{"schema":1,"kind":"discovered","name":"Camera"}'.encode("utf-16"),
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


def test_clearing_a_target_that_is_already_absent_succeeds(tmp_path):
    target = tmp_path / "source_target.json"
    save_source_target(target, None)
    assert not target.exists()
    save_source_target(target, SourceTarget("discovered", "Camera"))
    save_source_target(target, None)
    assert not target.exists()
    assert read_source_target(target) is None


def test_unopenable_lock_and_nonregular_lock_fail_closed(tmp_path, monkeypatch):
    target = tmp_path / "source_target.json"
    fifo = tmp_path / "source_target.json.lock"
    os.mkfifo(fifo)
    with pytest.raises(SourceConfigurationError, match="lock"):
        save_source_target(target, SourceTarget("discovered", "Camera"))
    fifo.unlink()

    monkeypatch.setattr(os, "open", raises(OSError("no descriptors")))
    with pytest.raises(SourceConfigurationError, match="unable to lock"):
        save_source_target(target, SourceTarget("discovered", "Camera"))


def test_underlying_os_errors_surface_as_configuration_errors(tmp_path, monkeypatch):
    target = tmp_path / "source_target.json"
    monkeypatch.setattr(
        "omt_client.state_store.atomic_replace",
        raises(OSError("disk full")),
    )
    with pytest.raises(SourceConfigurationError, match="disk full"):
        save_source_target(target, SourceTarget("discovered", "Camera"))


def test_an_encoded_target_over_the_record_limit_is_refused(tmp_path, monkeypatch):
    """Validated names and URIs always encode well under the limit, so this
    guard only fires if those bounds are ever relaxed. It must stay a typed
    configuration error rather than an opaque write failure."""
    target = tmp_path / "source_target.json"
    monkeypatch.setattr("omt_client.state_store.SOURCE_TARGET_MAX_BYTES", 8)
    with pytest.raises(SourceConfigurationError, match="oversized"):
        save_source_target(target, SourceTarget("discovered", "Camera"))
    assert not target.exists()


def test_source_target_oversized_and_nonregular_reads(tmp_path):
    path = tmp_path / "source_target.json"
    path.write_bytes(b"x" * 1025)
    with pytest.raises(SourceConfigurationError, match="unable to read"):
        read_source_target(path)
    path.unlink()
    path.mkdir()
    with pytest.raises(SourceConfigurationError, match="unable to read"):
        read_source_target(path)


def test_play_target_value_matches_saved_configuration(tmp_path):
    path = tmp_path / "source_target.json"
    save_source_target(path, SourceTarget("discovered", "Studio Camera"))
    assert play_target_value(path) == "Studio Camera"
    save_source_target(path, SourceTarget("direct", "omt://192.0.2.1:6400"))
    assert play_target_value(path) == "omt://192.0.2.1:6400"
    path.unlink()
    with pytest.raises(SystemExit, match="missing"):
        play_target_value(path)


def test_play_target_value_refuses_to_launch_on_a_corrupt_target(tmp_path):
    """`deploy/container/start-omt.sh` runs this and execs the receiver with
    whatever it prints. A corrupt record has to abort the launch with the reason
    on stderr; anything else would either exec an unvalidated target or print an
    error message as if it were one."""
    path = tmp_path / "source_target.json"
    path.write_text('{"schema":1,"kind":"discovered","name":"  bad  "}\n', encoding="utf-8")
    with pytest.raises(SystemExit, match="kind or value is invalid"):
        play_target_value(path)

    path.write_text("not json at all", encoding="utf-8")
    with pytest.raises(SystemExit, match="invalid JSON"):
        play_target_value(path)


def test_state_store_cli_prints_play_targets_and_rejects_bad_usage(tmp_path, capsys):
    from omt_client.state_store import main

    path = tmp_path / "source_target.json"
    save_source_target(path, SourceTarget("discovered", "Camera"))
    assert main(["play-target", str(path)]) == 0
    assert capsys.readouterr().out.strip() == "Camera"
    assert main(["wrong"]) == 2


# ─── Decode ceiling ──────────────────────────────────────────────────────────


def test_ceiling_grammar_matches_the_board_profile_table(tmp_path):
    """The four shipped tiers from `deploy/lib/board-profile.sh`.

    Three implementations parse this string -- the shell table, this module, and
    `omt_receiver_core::VideoCeiling` -- so each shipped default is asserted in
    all three. A tier that one of them rejects is an appliance that will not
    start.
    """
    for ceiling in (
        "1920x1080@60",
        "1920x1080@30,1280x720@60",
        "1280x720@60",
    ):
        assert parse_video_ceiling(ceiling) == ceiling


@pytest.mark.parametrize(
    "ceiling",
    [
        "",
        "1920x1080",
        "1920X1080@60",
        "1920x1080@60Hz",
        "1921x1080@60",
        "1920x1081@60",
        "1920x1080@61",
        "3840x2160@30",
        "15x15@60",
        "1920x1080@0",
        "0x1080@60",
        "0640x480@30",
        " 640x480@30",
        "1920x1080@60,",
        ",1920x1080@60",
        "1920x1080@60,,1280x720@30",
        "1920x1080@60 1280x720@30",
        "640x480@25,800x600@30,1280x720@50,1920x1080@30,640x360@24",
    ],
)
def test_ceiling_grammar_refuses_out_of_range_and_malformed_values(ceiling):
    """Nothing above 1920x1080@60 is representable, however it is spelled: that
    bound is what sizes the decoder's fixed allocations, and the operator
    override reaches this validator too."""
    with pytest.raises(VideoCeilingError):
        parse_video_ceiling(ceiling)


def test_ceiling_round_trips_and_clears(tmp_path):
    path = tmp_path / "video_ceiling.json"
    assert read_video_ceiling(path) is None

    save_video_ceiling(path, "1280x720@30")
    assert read_video_ceiling(path) == "1280x720@30"
    assert oct(path.stat().st_mode & 0o777) == "0o600"

    save_video_ceiling(path, None)
    assert read_video_ceiling(path) is None
    assert not path.exists()


def test_effective_ceiling_prefers_the_override_over_the_board_default(tmp_path):
    path = tmp_path / "video_ceiling.json"
    assert effective_video_ceiling(path, "1920x1080@30,1280x720@60") == ("1920x1080@30,1280x720@60")
    save_video_ceiling(path, "1280x720@30")
    assert effective_video_ceiling(path, "1920x1080@60") == "1280x720@30"


def test_corrupt_saved_ceiling_is_refused_rather_than_defaulted(tmp_path):
    """Falling back to a board default here would present the appliance as
    capable of something nobody chose, so every corruption is an exception."""
    path = tmp_path / "video_ceiling.json"

    path.write_text("not json at all", encoding="utf-8")
    with pytest.raises(VideoCeilingError, match="invalid JSON"):
        read_video_ceiling(path)

    path.write_text('{"schema":2,"ceiling":"1280x720@30"}\n', encoding="utf-8")
    with pytest.raises(VideoCeilingError, match="invalid schema"):
        read_video_ceiling(path)

    path.write_text('{"schema":1}\n', encoding="utf-8")
    with pytest.raises(VideoCeilingError, match="invalid schema"):
        read_video_ceiling(path)

    path.write_text('{"schema":1,"ceiling":3}\n', encoding="utf-8")
    with pytest.raises(VideoCeilingError, match="not a string"):
        read_video_ceiling(path)

    # A value that was valid when written but is out of range now.
    path.write_text('{"schema":1,"ceiling":"3840x2160@60"}\n', encoding="utf-8")
    with pytest.raises(VideoCeilingError):
        read_video_ceiling(path)


def test_an_unparseable_board_default_fails_rather_than_launching(tmp_path):
    """It arrives from the installer through the container environment. Failing
    here names the cause; the receiver's argument parser would not."""
    path = tmp_path / "video_ceiling.json"
    with pytest.raises(VideoCeilingError):
        effective_video_ceiling(path, "not-a-ceiling")


def test_ceiling_description_matches_the_receiver_wording():
    """`omt_receiver_core::VideoCeiling::describe` renders the same prose, so
    the dashboard and the status detail agree about what the limit is."""
    assert describe_video_ceiling("1920x1080@60") == "1920x1080 at 60 fps"
    assert describe_video_ceiling("1920x1080@30,1280x720@60") == (
        "1920x1080 at 30 fps, or 1280x720 at 60 fps"
    )


def test_state_store_cli_prints_the_effective_ceiling(tmp_path, capsys):
    from omt_client.state_store import main

    path = tmp_path / "video_ceiling.json"
    assert main(["video-ceiling", str(path), "1280x720@60"]) == 0
    assert capsys.readouterr().out.strip() == "1280x720@60"

    save_video_ceiling(path, "1280x720@30")
    assert main(["video-ceiling", str(path), "1280x720@60"]) == 0
    assert capsys.readouterr().out.strip() == "1280x720@30"

    assert main(["video-ceiling", str(path)]) == 2


def test_ceiling_description_degrades_rather_than_raising(tmp_path):
    """Rendering is used on a page that may be showing a corrupt saved value,
    so an unparseable ceiling is echoed rather than raised through the view."""
    assert describe_video_ceiling("nonsense") == "nonsense"
    assert describe_video_ceiling("1920x1080@60,broken") == "1920x1080@60,broken"


def test_unreadable_ceiling_file_is_an_error_not_an_absent_override(tmp_path):
    """A directory where the record should be reads as unreadable, not as
    'no override saved' -- otherwise a broken volume silently restores the
    board default."""
    path = tmp_path / "video_ceiling.json"
    path.mkdir()
    with pytest.raises(VideoCeilingError, match="unable to read"):
        read_video_ceiling(path)


def test_effective_ceiling_cli_helper_exits_with_the_reason(tmp_path):
    """`deploy/container/start-omt.sh` execs the receiver with whatever this
    prints, so a corrupt record has to abort the launch with the reason on
    stderr rather than print an error message as if it were a ceiling."""
    from omt_client.state_store import effective_ceiling_value

    path = tmp_path / "video_ceiling.json"
    path.write_text('{"schema":1,"ceiling":"3840x2160@60"}\n', encoding="utf-8")
    with pytest.raises(SystemExit, match="outside"):
        effective_ceiling_value(path, "1920x1080@60")
