"""Factory, injection, route, and redesigned-template tests."""

from __future__ import annotations

import re

import pytest
from conftest import REPO_ROOT, build_app, signed_in

from omt_client.models import ActionResult
from omt_client.settings import load_settings
from omt_client_preview import preview_services


@pytest.fixture
def factory_app():
    return build_app(services=preview_services("factory-password"))


@pytest.fixture
def factory_client(factory_app):
    return signed_in(factory_app, "factory-password")


def test_factory_stores_explicit_dependencies_and_defers_environment(factory_app):
    assert factory_app.extensions["omt_client.services"].auth.secret_key == "preview-dev-secret"
    assert factory_app.extensions["omt_client.settings"].config_dir == "/etc/omt"
    assert factory_app.config["SESSION_COOKIE_SECURE"] is False


def test_login_page_is_reachable_without_a_session(factory_app):
    response = factory_app.test_client().get("/login")
    assert response.status_code == 200
    assert b'name="password"' in response.data
    assert b"Invalid password" not in response.data


def test_login_failure_success_and_logout(factory_app):
    client = factory_app.test_client()
    assert b"Invalid password" in client.post("/login", data={"password": "bad"}).data
    assert client.post("/login", data={"password": "factory-password"}).status_code == 302
    assert client.get("/").status_code == 200
    assert client.post("/logout").status_code == 302
    assert client.get("/").headers["Location"].endswith("/login")


def test_authentication_storage_failures_fail_closed(factory_app, monkeypatch):
    auth = factory_app.extensions["omt_client.services"].auth

    def fail(*_args):
        raise OSError("storage unavailable")

    monkeypatch.setattr(auth, "authenticate", fail)
    client = factory_app.test_client()
    response = client.post("/login", data={"password": "factory-password"})
    assert response.status_code == 503
    assert b"Unable to create a persistent session" in response.data

    with client.session_transaction() as browser_session:
        browser_session["authenticated"] = True
        browser_session["session_id"] = "unavailable-session"
    monkeypatch.setattr(auth, "revoke", fail)
    response = client.post("/logout")
    assert response.status_code == 503
    assert b"Unable to revoke this session" in response.data


def test_dashboard_uses_one_heading_navigation_health_and_new_actions(factory_client):
    response = factory_client.get("/")
    html = response.get_data(as_text=True)
    assert response.status_code == 200
    assert len(re.findall(r"<h1(?:\s|>)", html)) == 1
    assert "Playback state" in html
    assert "Playing" in html
    assert "Network Settings" in html
    assert "Diagnostics" in html
    assert 'action="/sources/select"' in html
    assert 'action="/sources/refresh"' in html
    assert 'action="/playback/restart"' in html
    assert 'action="/playback/clear"' in html
    assert "Stop &amp; Clear Source" in html
    assert "OMT discovery" in html
    assert "System" in html
    assert "About" in html


def test_dashboard_actions_use_injected_source_service(factory_client, factory_app):
    source = factory_app.extensions["omt_client.services"].source
    selected = factory_client.post(
        "/sources/select",
        data={"source": "MEDIA-SERVER (Channel 1)"},
        follow_redirects=True,
    )
    assert b"Preview source saved and running" in selected.data
    assert source.configuration() == ("MEDIA-SERVER (Channel 1)", "")
    assert factory_client.post("/sources/refresh").status_code == 302
    assert (
        b"Preview playback restarted"
        in factory_client.post("/playback/restart", follow_redirects=True).data
    )
    cleared = factory_client.post("/playback/clear", follow_redirects=True)
    assert b"source cleared" in cleared.data
    assert source.configuration() == ("", "")
    assert (
        b"Invalid OMT source"
        in factory_client.post(
            "/sources/select", data={"source": "not-present"}, follow_redirects=True
        ).data
    )


def test_network_page_owns_sdk_and_direct_settings(factory_client, factory_app):
    page = factory_client.get("/settings/network")
    assert page.status_code == 200
    assert b"OMT discovery" in page.data
    assert b"Direct target" in page.data
    response = factory_client.post(
        "/settings/network",
        data={
            "discovery_server": "192.168.1.10:6399",
        },
        follow_redirects=True,
    )
    assert b"Preview network settings saved" in response.data
    network = factory_app.extensions["omt_client.services"].network.read()
    assert network["discovery_server"] == "192.168.1.10:6399"
    response = factory_client.post(
        "/settings/direct-source",
        data={"direct_address": "omt://192.168.1.50:6400"},
        follow_redirects=True,
    )
    assert b"Preview direct source saved" in response.data


@pytest.mark.parametrize(
    ("path", "title"),
    [
        ("/diagnostics/discovery", b"Discovery check"),
        ("/diagnostics/runtime", b"Runtime check"),
    ],
)
def test_separate_diagnostic_actions_render_typed_results(factory_client, path, title):
    response = factory_client.post(path)
    assert response.status_code == 200
    assert title in response.data
    assert b"Standard output" in response.data


def test_direct_diagnostic_and_bundle(factory_client):
    direct = factory_client.post(
        "/diagnostics/direct",
        data={"direct_address": "omt://192.0.2.4:6400"},
    )
    assert b"Direct-connect check" in direct.data
    bundle = factory_client.post("/diagnostics/download")
    assert bundle.status_code == 200
    assert bundle.mimetype == "application/zip"
    assert "omt-diagnostics-preview.zip" in bundle.headers["Content-Disposition"]


def test_legacy_debug_and_state_aliases_are_removed(factory_client):
    assert factory_client.get("/debug").status_code == 404
    assert factory_client.post("/debug").status_code == 404
    for path in ("/set_source", "/refresh", "/restart_service", "/debug/download"):
        assert factory_client.post(path).status_code == 404


def test_about_and_two_step_reboot_are_authenticated(factory_client):
    about = factory_client.get("/about")
    assert about.status_code == 200
    assert b"Copyright" in about.data
    assert b"Matthew David Miller" in about.data
    assert b"Project license" in about.data
    system = factory_client.get("/system")
    assert b"Reboot OS" in system.data
    confirmation = factory_client.get("/system/reboot")
    assert b"Confirm reboot" in confirmation.data
    scheduled = factory_client.post("/system/reboot")
    assert scheduled.status_code == 202
    assert b"reboot scheduled" in scheduled.data.lower()


def test_security_headers_and_contextual_error_pages(factory_client):
    response = factory_client.get("/")
    assert "script-src 'none'" in response.headers["Content-Security-Policy"]
    assert response.headers["Cache-Control"] == "no-store"
    oversized = factory_client.post(
        "/settings/direct-source",
        data={"direct_source": "x" * (17 * 1024)},
    )
    assert oversized.status_code == 413
    assert b"Request too large" in oversized.data
    assert b"Return to dashboard" in oversized.data


def test_authenticated_csrf_error_uses_operator_error_view():
    services = preview_services("csrf-password")
    application = build_app(services=services, WTF_CSRF_ENABLED=True)
    session_id = services.auth.authenticate("csrf-password", None)
    client = application.test_client()
    with client.session_transaction() as browser_session:
        browser_session["authenticated"] = True
        browser_session["session_id"] = session_id
    response = client.post("/playback/restart")
    assert response.status_code == 400
    assert b"Session expired" in response.data
    assert b"Return to dashboard" in response.data


def test_diagnostics_landing_page_renders_without_a_check(factory_client):
    response = factory_client.get("/diagnostics")
    assert response.status_code == 200
    assert b"running:4242" in response.data
    assert b"Standard output" not in response.data


def test_reboot_failure_reports_the_error_instead_of_scheduling(factory_client, factory_app):
    system = factory_app.extensions["omt_client.services"].system
    system.request_reboot = lambda: ActionResult(False, error="The host rejected the request.")
    response = factory_client.post("/system/reboot", follow_redirects=True)
    assert response.status_code == 200
    assert b"The host rejected the request." in response.data
    assert b"reboot scheduled" not in response.data.lower()


def test_network_save_failure_retains_the_submitted_value(factory_client, factory_app):
    network = factory_app.extensions["omt_client.services"].network
    network.save = lambda _server: ActionResult(False, error="Discovery Server is invalid.")
    response = factory_client.post(
        "/settings/network",
        data={"discovery_server": "not a host"},
    )
    assert response.status_code == 200
    assert b"Discovery Server is invalid." in response.data
    assert b"not a host" in response.data


def test_about_renders_the_shipped_legal_texts():
    """The About page is the product's license surface, so it must render the
    real LICENSE and THIRD_PARTY_NOTICES.txt that ship in the image."""
    settings = load_settings(
        {
            "OMT_PROJECT_LICENSE_FILE": str(REPO_ROOT / "LICENSE"),
            "OMT_THIRD_PARTY_NOTICES_FILE": str(REPO_ROOT / "THIRD_PARTY_NOTICES.txt"),
        }
    )
    client = signed_in(build_app(settings, preview_services("about-password")), "about-password")

    html = client.get("/about").get_data(as_text=True)
    assert (REPO_ROOT / "LICENSE").read_text(encoding="utf-8").strip().splitlines()[0] in html
    assert "unavailable in this image" not in html


def test_about_reports_missing_legal_files_without_failing():
    settings = load_settings({"OMT_PROJECT_LICENSE_FILE": "/nonexistent/LICENSE"})
    client = signed_in(build_app(settings, preview_services("legal-password")), "legal-password")
    response = client.get("/about")
    assert response.status_code == 200
    assert b"Project license is unavailable in this image." in response.data


def test_login_rate_limit_remains_context_specific():
    application = build_app(services=preview_services("limit-password"))
    client = application.test_client()
    for _index in range(5):
        client.post("/login", data={"password": "bad"})
    response = client.post("/login", data={"password": "bad"})
    assert response.status_code == 429
    assert b"Too many login attempts" in response.data


def test_unauthenticated_oversized_request_falls_back_to_the_login_view():
    application = build_app(services=preview_services("anon-password"))
    response = application.test_client().post("/login", data={"password": "x" * (17 * 1024)})
    assert response.status_code == 413
    assert b"Request is too large." in response.data
    assert b"Return to dashboard" not in response.data


@pytest.mark.parametrize(
    ("path", "form", "limit_setting", "limit"),
    [
        ("/diagnostics/discovery", {}, "OMT_DIAGNOSTICS_ACTION_LIMIT", 2),
        ("/diagnostics/runtime", {}, "OMT_DIAGNOSTICS_ACTION_LIMIT", 2),
        (
            "/diagnostics/direct",
            {"direct_address": "omt://192.0.2.4:6400"},
            "OMT_DIAGNOSTICS_ACTION_LIMIT",
            2,
        ),
        ("/diagnostics/download", {}, "OMT_DIAGNOSTICS_DOWNLOAD_LIMIT", 1),
        ("/system/reboot", {}, "OMT_REBOOT_ACTION_LIMIT", 1),
    ],
)
def test_expensive_endpoints_honour_their_configured_rate_limit(path, form, limit_setting, limit):
    """factory.py attaches these limits by rewriting app.view_functions after
    blueprint registration. Only /login was covered before, so a silent failure
    of that mechanism would have left every costly endpoint unthrottled."""
    client = signed_in(
        build_app(
            load_settings({limit_setting: f"{limit} per hour"}),
            preview_services("throttle-password"),
        ),
        "throttle-password",
    )

    for _attempt in range(limit):
        assert client.post(path, data=form).status_code in (200, 202)
    throttled = client.post(path, data=form)
    assert throttled.status_code == 429
    assert b"Too many requests" in throttled.data
    assert b"Too many login attempts" not in throttled.data


def _luminance(color: str) -> float:
    channels = [int(color[index : index + 2], 16) / 255 for index in (1, 3, 5)]
    adjusted = [
        value / 12.92 if value <= 0.04045 else ((value + 0.055) / 1.055) ** 2.4
        for value in channels
    ]
    return 0.2126 * adjusted[0] + 0.7152 * adjusted[1] + 0.0722 * adjusted[2]


def _contrast(first: str, second: str) -> float:
    bright, dark = sorted((_luminance(first), _luminance(second)), reverse=True)
    return (bright + 0.05) / (dark + 0.05)


@pytest.mark.parametrize(
    ("foreground", "background"),
    [
        ("#172033", "#f4f7fb"),
        ("#0b5cab", "#ffffff"),
        ("#146c43", "#ffffff"),
        ("#7a4900", "#ffffff"),
        ("#a61b1b", "#ffffff"),
        ("#edf3fb", "#0b1220"),
        ("#8bc2ff", "#141e2e"),
        ("#62d49a", "#141e2e"),
        ("#f1bd61", "#141e2e"),
        ("#ff8a8a", "#141e2e"),
    ],
)
def test_semantic_text_colors_meet_wcag_contrast(foreground, background):
    assert _contrast(foreground, background) >= 4.5
