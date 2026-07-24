"""Typed receiver-status and controlled playback failure behavior."""

from __future__ import annotations

import json
from datetime import UTC, datetime, timedelta
from pathlib import Path

import pytest

from omt_client.models import CommandResult
from omt_client.services.playback import PlaybackStatusRecord, RuntimeSourcePlayback
from omt_client.settings import load_settings
from omt_client.state_store import SourceConfigurationError


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


@pytest.mark.parametrize(
    "changes",
    [
        {"schema": 2},
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
        lambda *_args: (_ for _ in ()).throw(SourceConfigurationError("disk failed")),
    )
    assert "could not be cleared" in service.clear().error
    assert "disk failed" in service.select("discovered|Camera").error
