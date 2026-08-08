"""Routes wired to the real ServiceContainer rather than the preview fakes.

Every other route test injects `preview_services`, so the production
authentication path — password-to-session binding, cross-worker session
revocation, and fail-closed revalidation — is otherwise never exercised through
HTTP. These tests build the app with `production_services` against a temporary
config directory.
"""

from __future__ import annotations

import importlib
import os
import stat
import sys
from pathlib import Path

import pytest
from conftest import build_app, raises
from werkzeug.security import generate_password_hash

from omt_client.services import production_services
from omt_client.settings import load_settings

PASSWORD = "real-service-password"
# scrypt derivation costs ~85ms; the hash of a constant password is invariant, so
# deriving it once per session instead of per test keeps the suite fast.
PASSWORD_HASH = generate_password_hash(PASSWORD)


def _stub_command(path: Path, exit_code: int = 0, stdout: str = "") -> str:
    path.write_text(f"#!/bin/sh\nprintf '%s' {stdout!r}\nexit {exit_code}\n", encoding="utf-8")
    path.chmod(path.stat().st_mode | stat.S_IXUSR)
    return str(path)


@pytest.fixture
def real_settings(tmp_path: Path):
    config = tmp_path / "config"
    (config / "run").mkdir(parents=True)
    (config / "omt").mkdir()
    (config / "flask_secret").write_text("a" * 64, encoding="utf-8")
    (config / "web_password").write_text(PASSWORD_HASH, encoding="utf-8")
    return load_settings(
        {
            "OMT_CONFIG_DIR": str(config),
            # Mirrors control-omt.sh: prints "stopped" and exits 3 with no receiver.
            "OMT_CONTROL_COMMAND": _stub_command(tmp_path / "control", 3, "stopped\n"),
            "OMT_RECEIVER_COMMAND": _stub_command(tmp_path / "receiver", stdout="[]"),
            "RPI_OMT_CLIENT_VERSION_FILE": str(tmp_path / "version"),
            "OMT_PROJECT_LICENSE_FILE": str(tmp_path / "missing-license"),
            "OMT_THIRD_PARTY_NOTICES_FILE": str(tmp_path / "missing-notices"),
        }
    )


@pytest.fixture
def real_app(real_settings):
    return build_app(real_settings, production_services(real_settings))


def _login(client, password: str = PASSWORD):
    return client.post("/login", data={"password": password})


def test_login_binds_the_session_to_the_stored_password(real_app):
    client = real_app.test_client()
    assert _login(client, "wrong").status_code == 200
    assert _login(client).status_code == 302

    with client.session_transaction() as browser_session:
        assert browser_session["authenticated"] is True
        assert browser_session["session_id"]
        digest = browser_session["password_digest"]
    assert digest == real_app.extensions["omt_client.services"].auth.password_digest
    assert client.get("/").status_code == 200


def test_rotating_the_password_file_invalidates_live_sessions(real_app, real_settings):
    """routes/auth.py stores password_digest with no fallback; changing the
    password must strand every cookie issued against the old one."""
    client = real_app.test_client()
    assert _login(client).status_code == 302
    assert client.get("/").status_code == 200

    Path(real_settings.password_file).write_text(
        generate_password_hash("a-different-password"), encoding="utf-8"
    )
    rotated = build_app(real_settings, production_services(real_settings))
    rotated.secret_key = real_app.secret_key

    with rotated.test_client() as stale_client:
        stale_client.set_cookie("session", client.get_cookie("session").value)
        response = stale_client.get("/")
    assert response.status_code == 302
    assert response.headers["Location"].endswith("/login")


def test_logout_revokes_the_persistent_registry_entry(real_app):
    client = real_app.test_client()
    assert _login(client).status_code == 302
    with client.session_transaction() as browser_session:
        session_id = browser_session["session_id"]

    assert client.post("/logout").status_code == 302
    with client.session_transaction() as browser_session:
        browser_session["authenticated"] = True
        browser_session["session_id"] = session_id
        browser_session["password_digest"] = real_app.extensions[
            "omt_client.services"
        ].auth.password_digest
    replayed = client.get("/")
    assert replayed.status_code == 302
    assert replayed.headers["Location"].endswith("/login")


def test_unreadable_session_registry_fails_closed(real_app, monkeypatch):
    """authenticated() must clear the session and redirect when the registry
    cannot be validated, rather than trusting the cookie."""
    client = real_app.test_client()
    assert _login(client).status_code == 302

    auth = real_app.extensions["omt_client.services"].auth
    monkeypatch.setattr(auth, "is_current", raises(OSError("registry unavailable")))
    response = client.get("/")
    assert response.status_code == 302
    assert response.headers["Location"].endswith("/login")
    with client.session_transaction() as browser_session:
        assert "authenticated" not in browser_session


def test_session_registry_is_private_and_survives_a_second_container(real_app, real_settings):
    """Sessions live on disk so every gunicorn worker sees the same registry."""
    client = real_app.test_client()
    assert _login(client).status_code == 302

    registry = Path(real_settings.config_dir) / "web_sessions.json"
    assert stat.S_IMODE(registry.stat().st_mode) == 0o600

    second = build_app(real_settings, production_services(real_settings))
    second.secret_key = real_app.secret_key
    with second.test_client() as other_worker:
        other_worker.set_cookie("session", client.get_cookie("session").value)
        assert other_worker.get("/").status_code == 200


def test_dashboard_and_diagnostics_render_against_real_adapters(real_app):
    client = real_app.test_client()
    assert _login(client).status_code == 302

    dashboard = client.get("/")
    assert dashboard.status_code == 200
    assert b"No source configured" in dashboard.data

    diagnostics = client.get("/diagnostics")
    assert diagnostics.status_code == 200
    assert b"stopped" in diagnostics.data

    about = client.get("/about")
    assert about.status_code == 200
    assert b"Project license is unavailable in this image." in about.data


def test_missing_flask_secret_is_a_startup_error(real_settings):
    os.remove(Path(real_settings.config_dir) / "flask_secret")
    with pytest.raises(RuntimeError, match="Flask secret"):
        production_services(real_settings)


def test_gunicorn_entry_point_builds_an_app_from_the_environment(real_settings, monkeypatch):
    """`omt_client.wsgi:app` is what deploy/container/entrypoint.sh execs. It
    reads the environment at import time, so nothing else in the suite covers it."""
    monkeypatch.setenv("OMT_CONFIG_DIR", real_settings.config_dir)
    monkeypatch.setenv("OMT_CONTROL_COMMAND", real_settings.control_command)
    monkeypatch.setenv("OMT_RECEIVER_COMMAND", real_settings.receiver_command)
    monkeypatch.delitem(sys.modules, "omt_client.wsgi", raising=False)

    wsgi = importlib.import_module("omt_client.wsgi")
    importlib.reload(wsgi)
    try:
        assert wsgi.app.name == "omt_client"
        assert wsgi.app.config["SESSION_COOKIE_SECURE"] is True
        assert wsgi.app.test_client().get("/").status_code == 302
    finally:
        sys.modules.pop("omt_client.wsgi", None)


def _control_stub(path: Path, *, exit_code: int = 0, stdout: str = "ok\n") -> str:
    return _stub_command(path, exit_code, stdout)


@pytest.fixture
def mutable_real_app(tmp_path: Path):
    """Production services with control/receiver stubs that succeed for mutations."""
    config = tmp_path / "config"
    (config / "run").mkdir(parents=True)
    (config / "omt").mkdir()
    (config / "flask_secret").write_text("a" * 64, encoding="utf-8")
    (config / "web_password").write_text(PASSWORD_HASH, encoding="utf-8")
    host_actions = tmp_path / "host-actions"
    host_actions.mkdir()
    reboot_request = host_actions / "reboot.request"
    reboot_result = host_actions / "reboot.result"
    reboot_request.touch(mode=0o600)
    reboot_result.touch(mode=0o640)
    os.chmod(reboot_request, 0o600)
    diagnostics_request = tmp_path / "diagnostics.request"
    diagnostics_request.touch(mode=0o600)
    os.chmod(diagnostics_request, 0o600)
    discovery = '[{"name":"Camera","target":"Camera"}]'
    settings = load_settings(
        {
            "OMT_CONFIG_DIR": str(config),
            "OMT_CONTROL_COMMAND": _control_stub(tmp_path / "control", exit_code=0),
            "OMT_RECEIVER_COMMAND": _stub_command(tmp_path / "receiver", stdout=discovery),
            "RPI_OMT_CLIENT_VERSION_FILE": str(tmp_path / "version"),
            "OMT_PROJECT_LICENSE_FILE": str(tmp_path / "missing-license"),
            "OMT_THIRD_PARTY_NOTICES_FILE": str(tmp_path / "missing-notices"),
            "OMT_REBOOT_REQUEST_FILE": str(reboot_request),
            "OMT_REBOOT_RESULT_FILE": str(reboot_result),
            "OMT_REBOOT_ACK_TIMEOUT_SECONDS": "0.2",
            "OMT_DIAGNOSTICS_HOST_REQUEST_FILE": str(diagnostics_request),
            "OMT_DIAGNOSTICS_RECEIVE_PROBE": "0",
        }
    )
    (tmp_path / "version").write_text("vtest\n", encoding="utf-8")
    app = build_app(settings, production_services(settings))
    app.extensions["omt_client.test_reboot_request"] = reboot_request
    app.extensions["omt_client.test_reboot_result"] = reboot_result
    return app


def test_production_mutation_routes_with_stubbed_control(mutable_real_app, monkeypatch):
    client = mutable_real_app.test_client()
    assert _login(client).status_code == 302

    selected = client.post(
        "/sources/select",
        data={"source": "discovered|Camera"},
        follow_redirects=True,
    )
    assert selected.status_code == 200
    assert b"saved and running" in selected.data.lower() or b"OMT discovery" in selected.data

    assert client.post("/sources/refresh").status_code == 302
    assert client.post("/playback/restart", follow_redirects=True).status_code == 200
    cleared = client.post("/playback/clear", follow_redirects=True)
    assert cleared.status_code == 200

    network = client.post(
        "/settings/network",
        data={"discovery_server": ""},
        follow_redirects=True,
    )
    assert network.status_code == 200

    direct = client.post(
        "/settings/direct-source",
        data={"direct_address": "omt://192.0.2.10:6400"},
        follow_redirects=True,
    )
    assert direct.status_code == 200

    assert client.post("/diagnostics/discovery", follow_redirects=True).status_code == 200
    assert client.post("/diagnostics/runtime", follow_redirects=True).status_code == 200
    assert (
        client.post(
            "/diagnostics/direct",
            data={"direct_address": "omt://192.0.2.10:6400"},
            follow_redirects=True,
        ).status_code
        == 200
    )
    bundle = client.post("/diagnostics/download", data={"include_packet_capture": "0"})
    assert bundle.status_code == 200
    assert bundle.mimetype in {"application/zip", "application/x-zip-compressed"}

    reboot_request = mutable_real_app.extensions["omt_client.test_reboot_request"]
    reboot_result = mutable_real_app.extensions["omt_client.test_reboot_result"]

    def acknowledge_reboot(path: str, record: bytes) -> None:
        from omt_client.safe_io import write_fixed_inode

        write_fixed_inode(path, record, 512)
        request_id = dict(line.split("=", 1) for line in record.decode().splitlines())["request_id"]
        reboot_result.write_text(
            f"version=1\nrequest_id={request_id}\nstatus=accepted\ndetail=scheduled\n",
            encoding="utf-8",
        )

    monkeypatch.setattr(
        "omt_client.services.host_system.HostSystem._write_request",
        staticmethod(acknowledge_reboot),
    )
    accepted = client.post("/system/reboot")
    assert accepted.status_code == 202

    def reject_reboot(path: str, record: bytes) -> None:
        from omt_client.safe_io import write_fixed_inode

        write_fixed_inode(path, record, 512)
        request_id = dict(line.split("=", 1) for line in record.decode().splitlines())["request_id"]
        reboot_result.write_text(
            f"version=1\nrequest_id={request_id}\nstatus=rejected\ndetail=cooldown\n",
            encoding="utf-8",
        )

    monkeypatch.setattr(
        "omt_client.services.host_system.HostSystem._write_request",
        staticmethod(reject_reboot),
    )
    rejected = client.post("/system/reboot", follow_redirects=True)
    assert rejected.status_code == 200
    assert b"rejected" in rejected.data.lower() or b"cooldown" in rejected.data.lower()
    assert reboot_request.exists()


def test_corrupt_target_surfaces_on_diagnostics_and_network(mutable_real_app):
    client = mutable_real_app.test_client()
    assert _login(client).status_code == 302
    settings = mutable_real_app.extensions["omt_client.settings"]
    Path(settings.source_target_file).write_text("not-json", encoding="utf-8")

    diagnostics = client.get("/diagnostics")
    assert diagnostics.status_code == 200
    assert b"Saved OMT target is invalid" in diagnostics.data

    network = client.get("/settings/network")
    assert network.status_code == 200
    assert b"Saved OMT target is invalid" in network.data


def test_revoked_session_shows_no_nav_on_login(real_app):
    client = real_app.test_client()
    assert _login(client).status_code == 302
    with client.session_transaction() as browser_session:
        session_id = browser_session["session_id"]
    real_app.extensions["omt_client.services"].auth.revoke(session_id)
    response = client.get("/login")
    assert response.status_code == 200
    assert b"nav-strip" not in response.data
    assert b'name="password"' in response.data
