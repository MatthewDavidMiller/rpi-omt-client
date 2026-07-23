"""Production OMT service-boundary tests."""

from __future__ import annotations

import json
import os
import subprocess
from datetime import UTC, datetime
from pathlib import Path

import pytest
from flask import Flask, session
from omt_client.models import CommandResult
from omt_client.services import (
    HostSystem,
    PersistentAuthentication,
    RuntimeDiagnostics,
    RuntimeNetwork,
    RuntimeSourcePlayback,
    _atomic_write,
    _run,
    controller_pid,
    production_services,
)
from settings import load_settings
from werkzeug.security import generate_password_hash


def settings_for(tmp_path: Path, **overrides: str):
    (tmp_path / "run").mkdir(exist_ok=True)
    (tmp_path / "omt").mkdir(exist_ok=True)
    values = {
        "OMT_CONFIG_DIR": str(tmp_path),
        "OMT_RUNTIME_CONFIG_FILE": str(tmp_path / "omt/settings.xml"),
        "OMT_RECEIVER_COMMAND": "/receiver",
        "OMT_CONTROL_COMMAND": "/control",
        "RPI_OMT_CLIENT_VERSION_FILE": str(tmp_path / "version"),
        "OMT_RUNTIME_INTEGRITY_MANIFEST": str(tmp_path / "integrity"),
        "OMT_HOST_DEBUG_FILE": str(tmp_path / "host-debug"),
        "OMT_REBOOT_REQUEST_FILE": str(tmp_path / "reboot.request"),
        "OMT_REBOOT_RESULT_FILE": str(tmp_path / "reboot.result"),
        "OMT_REBOOT_ACK_TIMEOUT_SECONDS": "0.1",
    }
    values.update(overrides)
    return load_settings(values)


def command_result(returncode=0, stdout="", stderr="", error=""):
    return CommandResult(
        command="test",
        returncode=returncode,
        stdout=stdout,
        stderr=stderr,
        error=error,
    )


def test_persistent_authentication_hash_sessions_rotation_and_revocation(tmp_path):
    (tmp_path / "flask_secret").write_text("a" * 64, encoding="utf-8")
    (tmp_path / "web_password").write_text(
        generate_password_hash("correct"), encoding="utf-8"
    )
    auth = PersistentAuthentication(settings_for(tmp_path))
    assert auth.authenticate("wrong", None) is None
    first = auth.authenticate("correct", None)
    second = auth.authenticate("correct", first)
    assert first and second and first != second
    application = Flask(__name__)
    application.secret_key = "test"
    with application.test_request_context("/"):
        session.update(
            authenticated=True,
            session_id=second,
            password_digest=auth.password_digest,
        )
        assert auth.is_current()
        session["password_digest"] = "bad"
        assert not auth.is_current()
        session["password_digest"] = auth.password_digest
        auth.revoke(second)
        assert not auth.is_current()
        auth.revoke(None)


def test_persistent_authentication_rejects_missing_secret_and_supports_env_password(
    tmp_path, monkeypatch
):
    (tmp_path / "web_password").write_text("unused", encoding="utf-8")
    with pytest.raises(RuntimeError, match="Flask secret"):
        PersistentAuthentication(settings_for(tmp_path))
    (tmp_path / "flask_secret").write_text("secret", encoding="utf-8")
    monkeypatch.setenv("OMT_WEB_PASSWORD", "environment-secret")
    auth = PersistentAuthentication(settings_for(tmp_path))
    assert auth.authenticate("environment-secret", None)


def test_bounded_command_success_timeout_and_os_error(monkeypatch):
    result = _run(["/bin/sh", "-c", "printf ok; printf err >&2"], 2)
    assert result.returncode == 0 and result.stdout == "ok" and result.stderr == "err"

    def timeout(*_args, **_kwargs):
        raise subprocess.TimeoutExpired("receiver", 1, output=b"partial")

    monkeypatch.setattr(subprocess, "run", timeout)
    assert _run(["receiver"], 1).timed_out

    def missing(*_args, **_kwargs):
        raise FileNotFoundError("missing")

    monkeypatch.setattr(subprocess, "run", missing)
    assert "missing" in _run(["receiver"], 1).error


def test_atomic_write_is_private_bounded_and_rejects_unsafe_targets(tmp_path):
    target = tmp_path / "value"
    _atomic_write(str(target), b"safe", 4)
    assert target.read_bytes() == b"safe"
    assert target.stat().st_mode & 0o777 == 0o600
    with pytest.raises(OSError, match="exceeds"):
        _atomic_write(str(target), b"large", 4)
    target.unlink()
    target.symlink_to("elsewhere")
    with pytest.raises(OSError, match="regular"):
        _atomic_write(str(target), b"x", 4)


def test_source_discovery_cache_selection_direct_clear_and_restart(tmp_path, monkeypatch):
    settings = settings_for(tmp_path)
    calls: list[list[str]] = []

    def run(command, _timeout):
        calls.append(command)
        if "discover" in command:
            return command_result(
                stdout='[{"name":"Camera","target":"Camera"},{"name":"Bad","target":"Other"}]'
            )
        return command_result()

    monkeypatch.setattr("omt_client.services._run", run)
    service = RuntimeSourcePlayback(settings)
    assert [choice.name for choice in service.sources()] == ["Camera"]
    assert [choice.name for choice in service.sources()] == ["Camera"]
    assert len([call for call in calls if "discover" in call]) == 1
    assert not service.select("bad\nselection").ok
    assert service.select("discovered|Camera").ok
    assert service.configuration() == ("Camera", "")
    assert service.restart().ok
    assert not service.save_direct("", "host:6400").ok
    assert service.save_direct("", "omt://192.0.2.1:6400").ok
    assert service.configuration() == (
        "omt://192.0.2.1:6400",
        "omt://192.0.2.1:6400",
    )
    assert service.clear().ok
    assert service.configuration() == ("", "")
    assert not service.restart().ok


@pytest.mark.parametrize(
    ("runtime_state", "public_state"),
    [
        ("running", "playing"),
        ("waiting-for-hdmi", "waiting-for-hdmi"),
        ("retrying", "retrying"),
        ("degraded", "degraded"),
        ("unsupported-format", "unsupported-format"),
        ("starting", "starting"),
        ("stopped", "stopped"),
        ("failed", "failed"),
    ],
)
def test_playback_status_mapping(tmp_path, monkeypatch, runtime_state, public_state):
    settings = settings_for(tmp_path)
    service = RuntimeSourcePlayback(settings)
    Path(settings.source_target_file).write_text(
        '{"schema":1,"kind":"discovered","name":"Camera"}\n', encoding="utf-8"
    )
    Path(settings.playback_status_file).write_text(
        json.dumps(
            {
                "schema": 1,
                "state": runtime_state,
                "detail": "detail",
                "updated_at": datetime.now(UTC).isoformat(),
            }
        ),
        encoding="utf-8",
    )
    monkeypatch.setattr("omt_client.services._run", lambda *_args: command_result())
    assert service.playback().state == public_state


def test_network_round_trip_invalid_state_and_restart_failure(tmp_path, monkeypatch):
    settings = settings_for(tmp_path)
    source = RuntimeSourcePlayback(settings)
    network = RuntimeNetwork(settings, source)
    assert network.read()["discovery_server"] == ""
    assert network.save("discovery.example:6399").ok
    assert network.read()["discovery_server"] == "omt://discovery.example:6399"
    assert not network.save("bad host").ok
    Path(settings.source_target_file).write_text(
        '{"schema":1,"kind":"discovered","name":"Camera"}\n', encoding="utf-8"
    )
    monkeypatch.setattr(
        source,
        "restart",
        lambda: type("R", (), {"ok": False, "error": "failed"})(),
    )
    assert not network.save("").ok
    Path(settings.runtime_config_file).write_text("<wrong/>", encoding="utf-8")
    assert network.read()["error"]


def test_diagnostics_version_commands_direct_and_bundle(tmp_path, monkeypatch):
    settings = settings_for(tmp_path)
    Path(settings.version_file).write_text("v2.0.0\n", encoding="utf-8")
    Path(settings.runtime_config_file).write_text("<Settings />", encoding="utf-8")
    source = RuntimeSourcePlayback(settings)

    def run(command, _timeout):
        if "discover" in command:
            return command_result(stdout='[{"name":"Camera","target":"Camera"}]')
        return command_result(stdout="ok\n")

    monkeypatch.setattr("omt_client.services._run", run)
    diagnostics = RuntimeDiagnostics(settings, source)
    assert diagnostics.version() == "v2.0.0"
    assert diagnostics.status() == "ok"
    assert diagnostics.discovery().command.sources == ("Camera",)
    assert diagnostics.runtime().command.returncode == 0
    assert diagnostics.direct("", "bad").command.skipped
    assert diagnostics.direct("", "omt://host:6400").command.returncode == 0
    bundle, filename = diagnostics.bundle()
    assert filename.startswith("omt-debug-")
    assert bundle.read(2) == b"PK"


def test_host_system_accept_reject_timeout_and_unsafe_request(tmp_path, monkeypatch):
    settings = settings_for(tmp_path)
    request = Path(settings.reboot_request_file)
    result = Path(settings.reboot_result_file)
    request.touch(mode=0o600)
    result.touch(mode=0o640)
    os.chmod(request, 0o600)
    original_write_request = HostSystem._write_request

    def acknowledge(_path: str, record: bytes):
        request_id = dict(
            line.split("=", 1) for line in record.decode().splitlines()
        )["request_id"]
        result.write_text(
            f"version=1\nrequest_id={request_id}\nstatus=accepted\ndetail=scheduled\n",
            encoding="utf-8",
        )

    monkeypatch.setattr(HostSystem, "_write_request", staticmethod(acknowledge))
    assert HostSystem(settings).request_reboot().ok

    def reject(_path: str, record: bytes):
        request_id = dict(
            line.split("=", 1) for line in record.decode().splitlines()
        )["request_id"]
        result.write_text(
            f"version=1\nrequest_id={request_id}\nstatus=rejected\ndetail=cooldown\n",
            encoding="utf-8",
        )

    monkeypatch.setattr(HostSystem, "_write_request", staticmethod(reject))
    assert "cooldown" in HostSystem(settings).request_reboot().error
    monkeypatch.setattr(
        HostSystem, "_write_request", staticmethod(lambda *_args: None)
    )
    result.write_text("", encoding="utf-8")
    assert "did not acknowledge" in HostSystem(settings).request_reboot().error
    request.chmod(0o644)
    with pytest.raises(OSError, match="unsafe"):
        original_write_request(str(request), b"request")


def test_production_container_and_controller_pid(tmp_path, monkeypatch):
    (tmp_path / "flask_secret").write_text("secret", encoding="utf-8")
    (tmp_path / "web_password").write_text(
        generate_password_hash("password"), encoding="utf-8"
    )
    services = production_services(settings_for(tmp_path))
    assert isinstance(services.auth, PersistentAuthentication)
    assert controller_pid("running:42 state=running") == 42
    assert controller_pid("stopped") is None
