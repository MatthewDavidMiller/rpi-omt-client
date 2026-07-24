"""Side-effect-free preview service behavior."""

import re
import subprocess
import sys
import zipfile

from conftest import REPO_ROOT
from flask import Flask, session

from omt_client_preview import preview_services

SHIPPED_PACKAGE = REPO_ROOT / "src" / "omt_client"


def test_shipped_package_never_depends_on_the_preview_fakes():
    """deploy/Dockerfile copies only src/omt_client/, so nothing there may import
    the dev-only fakes — that would break the appliance image at runtime."""
    dockerfile = (REPO_ROOT / "deploy" / "Dockerfile").read_text(encoding="utf-8")
    assert "COPY src/omt_client/ /app/omt_client/" in dockerfile
    assert "omt_client_preview" not in dockerfile

    offenders = [
        module.relative_to(REPO_ROOT).as_posix()
        for module in SHIPPED_PACKAGE.rglob("*.py")
        if "omt_client_preview" in module.read_text(encoding="utf-8")
    ]
    assert not offenders, f"Preview fakes leaked into the shipped package: {offenders}"


def test_dockerignore_excludes_bytecode_at_every_depth():
    """An unanchored `__pycache__/` only matches the build-context root, so
    nested bytecode was copied into the image and hashed into
    runtime-sha256.manifest, making the manifest depend on whatever the build
    machine had compiled locally."""
    patterns = [
        line.strip()
        for line in (REPO_ROOT / ".dockerignore").read_text(encoding="utf-8").splitlines()
        if line.strip() and not line.startswith("#")
    ]
    assert "**/__pycache__/" in patterns
    assert "**/*.pyc" in patterns
    assert "__pycache__/" not in patterns
    assert "*.pyc" not in patterns


def test_preview_password_digest_matches_the_production_binding():
    """routes/auth.py stores auth.password_digest with no fallback, so every
    AuthenticationService implementation must supply a real one."""
    services = preview_services("secret")
    digest = services.auth.password_digest
    assert re.fullmatch(r"[0-9a-f]{64}", digest)
    assert digest != preview_services("other").auth.password_digest


def test_preview_launcher_imports_package_from_src(tmp_path):
    launcher = REPO_ROOT / "scripts" / "preview-web-ui.py"
    command = f"import runpy; runpy.run_path({str(launcher)!r}, run_name='preview_import_test')"
    completed = subprocess.run(
        [sys.executable, "-I", "-c", command],
        cwd=tmp_path,
        capture_output=True,
        text=True,
        check=False,
    )
    assert completed.returncode == 0, completed.stderr


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
    assert services.source.save_direct("bad").ok is False
    assert services.source.save_direct("omt://host:6400").ok is True
    assert services.network.save("omt://192.0.2.1:6399").ok is True
    assert services.network.read()["discovery_server"] == "omt://192.0.2.1:6399"
    assert services.diagnostics.version() == "preview"
    assert "running:" in services.diagnostics.status()
    assert services.diagnostics.discovery().command.sources
    assert services.diagnostics.runtime().command.returncode == 0
    assert "omt://host:6400" in services.diagnostics.direct("omt://host:6400").command.command
    assert services.system.request_reboot().ok
    bundle, name = services.diagnostics.bundle()
    assert name.endswith(".zip")
    with zipfile.ZipFile(bundle) as archive:
        assert "Preview bundle" in archive.read("preview.txt").decode()
