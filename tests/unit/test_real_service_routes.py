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
from werkzeug.security import generate_password_hash

from omt_client import create_app
from omt_client.services import production_services
from omt_client.settings import load_settings

PASSWORD = "real-service-password"


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
    (config / "web_password").write_text(generate_password_hash(PASSWORD), encoding="utf-8")
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
    application = create_app(real_settings, production_services(real_settings))
    application.config.update(
        TESTING=True,
        WTF_CSRF_ENABLED=False,
        SESSION_COOKIE_SECURE=False,
    )
    return application


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
    rotated = create_app(real_settings, production_services(real_settings))
    rotated.config.update(TESTING=True, WTF_CSRF_ENABLED=False, SESSION_COOKIE_SECURE=False)
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


def test_unreadable_session_registry_fails_closed(real_app, real_settings, monkeypatch):
    """authenticated() must clear the session and redirect when the registry
    cannot be validated, rather than trusting the cookie."""
    client = real_app.test_client()
    assert _login(client).status_code == 302

    auth = real_app.extensions["omt_client.services"].auth
    monkeypatch.setattr(
        auth, "is_current", lambda: (_ for _ in ()).throw(OSError("registry unavailable"))
    )
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

    second = create_app(real_settings, production_services(real_settings))
    second.config.update(TESTING=True, WTF_CSRF_ENABLED=False, SESSION_COOKIE_SECURE=False)
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
