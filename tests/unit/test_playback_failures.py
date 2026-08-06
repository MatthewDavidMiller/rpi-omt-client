"""Typed receiver-status and controlled playback failure behavior."""

from __future__ import annotations

import json
from datetime import UTC, datetime, timedelta
from pathlib import Path

import pytest
from conftest import REPO_ROOT, raises

from omt_client.models import CommandResult
from omt_client.playback_status import (
    AUDIO_STATES,
    CONNECTORS,
    PUBLIC_STATES,
    RECEIVER_STATES,
    STATUS_FIELDS,
    VIDEO_STATES,
    PlaybackStatusRecord,
)
from omt_client.services.playback import RuntimeSourcePlayback
from omt_client.settings import load_settings
from omt_client.state_store import SourceConfigurationError

VECTORS = json.loads(
    (REPO_ROOT / "tests" / "schema" / "playback-status-vectors.json").read_text(encoding="utf-8")
)


def _status_document(**changes):
    document = {
        "schema": 1,
        "state": "running",
        "video_state": "running",
        "audio_state": "running",
        "target": "Camera",
        "detail": "playing",
        "connector": "HDMI-A-1",
        "drm_device": "/dev/dri/card1",
        "alsa_device": "plughw:0",
        "updated_at": datetime.now(UTC).isoformat(),
    }
    document.update(changes)
    return document


def _service(tmp_path: Path) -> RuntimeSourcePlayback:
    (tmp_path / "run").mkdir(exist_ok=True)
    settings = load_settings(
        {
            "OMT_CONFIG_DIR": str(tmp_path),
            "OMT_CONTROL_COMMAND": "/control",
            "OMT_RECEIVER_COMMAND": "/receiver",
            "OMT_PLAYBACK_STATUS_STALE_SECONDS": "5",
        }
    )
    Path(settings.source_target_file).write_text(
        '{"schema":1,"kind":"discovered","name":"Camera"}\n',
        encoding="utf-8",
    )
    return RuntimeSourcePlayback(settings)


def test_consumer_accept_lists_match_the_shared_receiver_contract():
    """The C producer asserts against the same file. If these drift, Python's
    exact `set(document) == STATUS_FIELDS` check rejects every real status
    record and the dashboard silently pins to 'Playback status stale'."""
    assert STATUS_FIELDS == set(VECTORS["fields"])
    assert RECEIVER_STATES == set(VECTORS["receiver_states"])
    assert VIDEO_STATES == set(VECTORS["video_states"])
    assert AUDIO_STATES == set(VECTORS["audio_states"])
    # Parametrising over VECTORS["connectors"] only proves the vectors are
    # accepted. Equality also proves the reverse: a connector the receiver can
    # never publish cannot quietly widen the consumer's accept-list.
    assert CONNECTORS == set(VECTORS["connectors"])
    assert _status_document()["schema"] == VECTORS["schema"]
    # playback() indexes PUBLIC_STATES directly. Only states with a projection row
    # are exercised end to end, so totality has to be asserted rather than sampled.
    assert set(PUBLIC_STATES) == RECEIVER_STATES


@pytest.mark.parametrize("vector", VECTORS["projections"], ids=lambda case: case["name"])
def test_every_producible_projection_parses_and_maps_to_a_public_state(
    tmp_path: Path, vector: dict[str, str]
):
    service = _service(tmp_path)
    document = _status_document(
        state=vector["state"],
        video_state=vector["video_state"],
        audio_state=vector["audio_state"],
        detail=vector["name"],
    )
    parsed = PlaybackStatusRecord.parse(json.dumps(document).encode())
    assert (parsed.state, parsed.video_state, parsed.audio_state) == (
        vector["state"],
        vector["video_state"],
        vector["audio_state"],
    )

    Path(service._settings.playback_status_file).write_text(json.dumps(document), encoding="utf-8")
    summary = service.playback()
    assert summary.state == vector["public_state"]
    assert summary.tone == vector["tone"]
    assert summary.detail == vector["name"]


@pytest.mark.parametrize("connector", VECTORS["connectors"])
def test_every_contracted_connector_is_accepted(connector: str):
    document = _status_document(connector=connector)
    assert PlaybackStatusRecord.parse(json.dumps(document).encode()).state == "running"


@pytest.mark.parametrize(
    "changes",
    [
        {"schema": 2},
        {"schema": True},
        {"schema": 1.0},
        {"state": "unknown"},
        {"detail": 1},
        {"updated_at": "bad"},
        {"updated_at": "2026-01-01T00:00:00"},
        {"state": "degraded", "audio_state": "running"},
        {"state": "running", "audio_state": "failed"},
        {"state": "retrying", "video_state": "running"},
        {"target": "bad\nname"},
        {"connector": "HDMI-A-3"},
    ],
)
def test_status_schema_rejects_malformed_records(changes):
    with pytest.raises(ValueError):
        PlaybackStatusRecord.parse(json.dumps(_status_document(**changes)).encode())


@pytest.mark.parametrize("document", [b"not json", b"[]", b'{"schema":1}'])
def test_status_schema_rejects_non_documents(document: bytes):
    with pytest.raises(ValueError):
        PlaybackStatusRecord.parse(document)


def test_status_schema_rejects_duplicate_keys_and_non_utf8_encoding():
    raw = json.dumps(_status_document()).replace('"schema": 1', '"schema": 2, "schema": 1')
    with pytest.raises(ValueError, match="valid JSON"):
        PlaybackStatusRecord.parse(raw.encode())
    with pytest.raises(ValueError, match="valid JSON"):
        PlaybackStatusRecord.parse(json.dumps(_status_document()).encode("utf-16"))


def test_status_schema_rejects_oversized_detail():
    document = json.dumps(_status_document(detail="é" * 1025)).encode()
    with pytest.raises(ValueError, match="fields"):
        PlaybackStatusRecord.parse(document)


@pytest.mark.parametrize(
    ("offset", "expected"),
    [
        (timedelta(seconds=-30), "stale"),
        (timedelta(seconds=30), "stale"),
        (timedelta(), "waiting-for-discovery"),
    ],
)
def test_playback_rejects_stale_future_and_presents_discovery_wait(
    tmp_path: Path, offset: timedelta, expected: str
):
    service = _service(tmp_path)
    Path(service._settings.playback_status_file).write_text(
        json.dumps(
            _status_document(
                state="waiting-for-discovery",
                video_state="waiting-for-discovery",
                detail="waiting",
                updated_at=(datetime.now(UTC) + offset).isoformat(),
            )
        ),
        encoding="utf-8",
    )
    assert service.playback().state == expected


@pytest.mark.parametrize(("returncode", "expected"), [(0, "starting"), (3, "stopped")])
def test_missing_status_uses_controlled_controller_fallback(
    tmp_path: Path, monkeypatch, returncode: int, expected: str
):
    service = _service(tmp_path)
    monkeypatch.setattr(
        service,
        "_control",
        lambda _action: CommandResult(command="control", returncode=returncode),
    )
    assert service.playback().state == expected


def test_lone_surrogate_detail_is_rejected_rather_than_crashing():
    """json.loads happily produces unpaired surrogates, which raise on UTF-8
    encode. The bounded-text guard must treat that as an invalid field."""
    raw = json.dumps(_status_document()).replace('"detail": "playing"', '"detail": "\\ud800"')
    with pytest.raises(ValueError, match="fields"):
        PlaybackStatusRecord.parse(raw.encode())


def test_unreadable_saved_target_is_not_mislabeled_as_unconfigured(tmp_path: Path):
    service = _service(tmp_path)
    Path(service._settings.source_target_file).write_text("not-json", encoding="utf-8")
    assert service.configuration() == ("", "")
    summary = service.playback()
    assert summary.state == "configuration-error"
    assert summary.tone == "danger"
    assert "invalid JSON" in summary.detail


def test_fresh_status_for_a_previous_target_is_rejected(tmp_path: Path):
    """A source switch can leave the old receiver record fresh for a few seconds."""
    service = _service(tmp_path)
    Path(service._settings.playback_status_file).write_text(
        json.dumps(_status_document(target="Previous Camera")),
        encoding="utf-8",
    )
    summary = service.playback()
    assert summary.state == "stale"
    assert summary.source == "Camera"


def test_restart_failures_surface_the_controller_detail(tmp_path: Path, monkeypatch):
    service = _service(tmp_path)
    monkeypatch.setattr(
        service,
        "_control",
        lambda _action: CommandResult(command="control", returncode=1, stderr="no such device"),
    )
    restarted = service.restart()
    assert not restarted.ok
    assert "no such device" in restarted.error

    saved = service.select("discovered|Camera")
    assert not saved.ok
    assert "was saved, but playback could not be restarted" in saved.error
    assert "no such device" in saved.error


def test_malformed_persisted_target_restart_is_a_controlled_error(tmp_path: Path):
    service = _service(tmp_path)
    Path(service._settings.source_target_file).write_text("not-json", encoding="utf-8")
    result = service.restart()
    assert not result.ok
    assert "Saved OMT target is invalid" in result.error


def test_stop_and_persistence_failures_retain_or_report_state(tmp_path: Path, monkeypatch):
    service = _service(tmp_path)
    monkeypatch.setattr(
        service,
        "_control",
        lambda _action: CommandResult(command="control", returncode=1, stderr="stop failed"),
    )
    result = service.clear()
    assert not result.ok and "retained" in result.error
    assert Path(service._settings.source_target_file).exists()

    monkeypatch.setattr(
        service,
        "_control",
        lambda _action: CommandResult(command="control", returncode=0),
    )
    monkeypatch.setattr(
        "omt_client.services.playback.save_source_target",
        raises(SourceConfigurationError("disk failed")),
    )
    assert "could not be cleared" in service.clear().error
    assert "disk failed" in service.select("discovered|Camera").error
