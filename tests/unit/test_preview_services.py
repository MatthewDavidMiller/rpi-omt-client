"""Side-effect-free preview service behavior."""

import zipfile

from flask import Flask, session
from omt_client.preview import preview_services


def test_preview_authentication_rotates_and_revokes_sessions():
    services = preview_services("secret")
    assert services.auth.authenticate("bad", None) is None
    first = services.auth.authenticate("secret", None)
    second = services.auth.authenticate("secret", first)
    app = Flask(__name__)
    app.secret_key = "test"
    with app.test_request_context("/"):
        session["session_id"] = first
        assert services.auth.is_current() is False
        session["session_id"] = second
        assert services.auth.is_current() is True
        services.auth.revoke(second)
        assert services.auth.is_current() is False
        services.auth.revoke(None)


def test_preview_source_network_diagnostics_and_bundle_are_in_memory():
    services = preview_services()
    assert len(services.source.sources()) == 5
    assert services.source.playback().state == "playing"
    assert services.source.select("missing").ok is False
    assert services.source.restart().ok is True
    assert services.source.clear().ok is True
    assert services.source.playback().state == "unconfigured"
    assert services.source.restart().ok is False
    assert services.source.save_direct("", "bad").ok is False
    assert services.source.save_direct("CAM", "omt://host:6400").ok is True
    assert services.network.save("omt://192.0.2.1:6399").ok is True
    assert services.network.read()["discovery_server"] == "omt://192.0.2.1:6399"
    assert services.diagnostics.version() == "preview"
    assert "running:" in services.diagnostics.status()
    assert services.diagnostics.discovery().command.sources
    assert services.diagnostics.runtime().command.returncode == 0
    assert "omt://host:6400" in services.diagnostics.direct("CAM", "omt://host:6400").command.command
    assert services.system.request_reboot().ok
    bundle, name = services.diagnostics.bundle()
    assert name.endswith(".zip")
    with zipfile.ZipFile(bundle) as archive:
        assert "Preview bundle" in archive.read("preview.txt").decode()
