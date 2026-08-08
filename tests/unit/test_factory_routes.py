"""Factory, injection, route, and redesigned-template tests."""

from __future__ import annotations

import re

import pytest
from conftest import REPO_ROOT, build_app, signed_in
from flask import abort

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
    assert client.get("/login").status_code == 302
    assert client.get("/login").headers["Location"].endswith("/")
    assert client.post("/logout").status_code == 302
    assert client.get("/").headers["Location"].endswith("/login")


def test_forged_authenticated_cookie_shows_no_nav_on_login(factory_app):
    client = factory_app.test_client()
    with client.session_transaction() as browser_session:
        browser_session["authenticated"] = True
        browser_session["session_id"] = "forged-session"
        browser_session["password_digest"] = "deadbeef"
    response = client.get("/login")
    assert response.status_code == 200
    assert b'name="password"' in response.data
    assert b"nav-strip" not in response.data
    assert b"Log out" not in response.data
    assert b"Logout" not in response.data


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
    assert b"Signed out locally" in response.data
    assert client.get("/").headers["Location"].endswith("/login")


def test_malformed_old_session_ids_do_not_block_login_or_logout(factory_app):
    client = factory_app.test_client()
    with client.session_transaction() as browser_session:
        browser_session["session_id"] = 42
    assert client.post("/login", data={"password": "factory-password"}).status_code == 302
    with client.session_transaction() as browser_session:
        browser_session["session_id"] = 42
    assert client.post("/logout").status_code == 302


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


def test_dashboard_uses_the_playback_snapshot_for_current_source(
    factory_client,
    factory_app,
    monkeypatch,
):
    """Do not reread target state after playback has already captured it."""
    source = factory_app.extensions["omt_client.services"].source

    def duplicate_read():
        raise AssertionError("dashboard performed a second configuration read")

    monkeypatch.setattr(source, "configuration", duplicate_read)
    response = factory_client.get("/")
    assert response.status_code == 200
    assert b"STUDIO-PC (OBS Studio)" in response.data


def test_dashboard_renders_sources_through_the_typed_choice_contract(
    factory_client,
    factory_app,
):
    """The template used to hedge every attribute against a bare string, from
    when `sources()` returned names. It now reads `SourceChoice` directly, so
    the option value, label, and badge must all come from the choice itself --
    a service returning something thinner is a type error, not a blank badge."""
    html = factory_client.get("/").get_data(as_text=True)
    for choice in factory_app.extensions["omt_client.services"].source.sources():
        assert f'value="{choice.selection_value}"' in html
        assert f">{choice.display_label}</option>" in html
        assert f'<span class="badge">{choice.backend}</span> {choice.name}' in html


def test_dashboard_actions_use_injected_source_service(factory_client, factory_app):
    source = factory_app.extensions["omt_client.services"].source
    selected = factory_client.post(
        "/sources/select",
        data={"source": "MEDIA-SERVER (Channel 1)"},
        follow_redirects=True,
    )
    assert b"Preview source saved and running" in selected.data
    assert source.configuration().source == "MEDIA-SERVER (Channel 1)"
    assert factory_client.post("/sources/refresh").status_code == 302
    assert (
        b"Preview playback restarted"
        in factory_client.post("/playback/restart", follow_redirects=True).data
    )
    cleared = factory_client.post("/playback/clear", follow_redirects=True)
    assert b"source cleared" in cleared.data
    assert source.configuration().source == ""
    assert not source.configuration().configured
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
    assert network["discovery_server"] == "omt://192.168.1.10:6399"
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


def test_templates_stay_within_the_content_security_policy():
    """The response header sends `style-src 'self'; script-src 'none'`, so an
    inline style, script, or event handler added to a template is dropped by
    the browser with no server-side error: the page just renders unstyled or
    inert. Keep the markup and the policy from drifting apart."""
    blocked = re.compile(r"<script\b|\sstyle=|\son[a-z]+=|javascript:", re.IGNORECASE)
    offenders = [
        f"{template.name}:{line_number}"
        for template in sorted((REPO_ROOT / "src" / "omt_client" / "templates").glob("*.html"))
        for line_number, line in enumerate(template.read_text(encoding="utf-8").splitlines(), 1)
        if blocked.search(line)
    ]
    assert not offenders, "Templates use markup the CSP blocks: " + ", ".join(offenders)


def test_unknown_paths_render_the_context_appropriate_view(factory_app, factory_client):
    """Without a 404 handler these served Werkzeug's default page, leaking
    framework markup to an unauthenticated caller."""
    signed_out = factory_app.test_client().get("/no-such-page")
    assert signed_out.status_code == 404
    assert b"That page does not exist." in signed_out.data
    assert b"Password" in signed_out.data
    assert b"Werkzeug" not in signed_out.data

    signed_in_response = factory_client.get("/no-such-page")
    assert signed_in_response.status_code == 404
    assert b"Page not found" in signed_in_response.data
    assert b"Return to dashboard" in signed_in_response.data


def test_wrong_method_renders_the_operator_error_view(factory_client):
    response = factory_client.get("/playback/restart")
    assert response.status_code == 405
    assert b"Action not allowed" in response.data
    assert b"Return to dashboard" in response.data


def test_unhandled_route_failure_is_an_opaque_logged_500(factory_app, factory_client, caplog):
    """The exception text must never reach the browser, but it must be logged."""
    source = factory_app.extensions["omt_client.services"].source

    def explode():
        raise RuntimeError("secret-internal-detail")

    source.playback = explode
    response = factory_client.get("/")
    assert response.status_code == 500
    assert b"Something went wrong" in response.data
    assert b"secret-internal-detail" not in response.data
    assert "secret-internal-detail" in caplog.text


def test_error_page_failure_falls_back_to_plaintext(factory_app, factory_client, monkeypatch):
    """A failure while rendering the operator error page must stay opaque."""
    source = factory_app.extensions["omt_client.services"].source

    def explode():
        raise RuntimeError("primary")

    source.playback = explode

    def broken_render(*_args, **_kwargs):
        raise RuntimeError("secondary")

    monkeypatch.setattr("omt_client.factory.render_template", broken_render)
    response = factory_client.get("/")
    assert response.status_code == 500
    assert b"Internal Server Error" in response.data
    assert b"primary" not in response.data
    assert b"secondary" not in response.data


def test_package_exports_are_lazy():
    import omt_client

    assert callable(omt_client.create_app)
    assert omt_client.ActionResult is not None
    assert omt_client.ServiceContainer is not None
    with pytest.raises(AttributeError):
        _ = omt_client.not_an_export


def test_aborted_http_errors_keep_their_status_and_message():
    """An HTTPException with no dedicated handler falls through the catch-all,
    which must preserve its code rather than reporting everything as a 500."""
    application = build_app(services=preview_services("abort-password"))

    @application.get("/forbidden-probe")
    def forbidden_probe():
        abort(403)

    response = signed_in(application, "abort-password").get("/forbidden-probe")
    assert response.status_code == 403
    assert b"Forbidden" in response.data


def test_http_error_page_failure_keeps_the_original_status(monkeypatch):
    """The catch-all guards its own template call, so a broken error page still
    answers with the status the request earned. Falling through to Flask's
    default here would report a 403 as a 500 and hand the operator a misleading
    cause for the failure they are chasing."""
    application = build_app(services=preview_services("render-password"))

    @application.get("/forbidden-probe")
    def forbidden_probe():
        abort(403)

    client = signed_in(application, "render-password")

    def broken_render(*_args, **_kwargs):
        raise RuntimeError("template blew up")

    monkeypatch.setattr("omt_client.factory.render_template", broken_render)
    response = client.get("/forbidden-probe")
    assert response.status_code == 403
    assert b"template blew up" not in response.data


def test_static_assets_are_cacheable_but_keep_transport_security(factory_client):
    response = factory_client.get("/static/style.css")
    assert response.status_code == 200
    assert "Cache-Control" in response.headers
    assert response.headers["Cache-Control"] != "no-store"
    assert "Pragma" not in response.headers
    assert response.headers["X-Content-Type-Options"] == "nosniff"
    assert "script-src 'none'" in response.headers["Content-Security-Policy"]


def test_authenticated_csrf_error_uses_operator_error_view():
    services = preview_services("csrf-password")
    application = build_app(services=services, WTF_CSRF_ENABLED=True)
    session_id = services.auth.authenticate("csrf-password", None)
    client = application.test_client()
    with client.session_transaction() as browser_session:
        browser_session["authenticated"] = True
        browser_session["session_id"] = session_id
        browser_session["password_digest"] = services.auth.password_digest
    response = client.post("/playback/restart")
    assert response.status_code == 400
    assert b"Session expired" in response.data
    assert b"Return to dashboard" in response.data


@pytest.mark.parametrize(
    "path",
    [
        "/sources/select",
        "/sources/refresh",
        "/playback/restart",
        "/playback/clear",
        "/settings/network",
        "/settings/direct-source",
        "/diagnostics/discovery",
        "/diagnostics/runtime",
        "/diagnostics/direct",
        "/diagnostics/download",
        "/system/reboot",
        "/logout",
    ],
)
def test_mutating_posts_require_csrf_when_enabled(path):
    services = preview_services("csrf-matrix")
    application = build_app(services=services, WTF_CSRF_ENABLED=True)
    client = application.test_client()
    login_page = client.get("/login").get_data(as_text=True)
    login_token = re.search(r'name="csrf_token"[^>]*value="([^"]+)"', login_page)
    assert login_token is not None
    assert (
        client.post(
            "/login",
            data={"password": "csrf-matrix", "csrf_token": login_token.group(1)},
        ).status_code
        == 302
    )
    response = client.post(
        path,
        data={"source": "x", "discovery_server": "x", "direct_address": "x"},
    )
    assert response.status_code == 400
    assert b"Session expired" in response.data


@pytest.mark.parametrize(
    ("method", "path"),
    [
        ("GET", "/"),
        ("GET", "/settings/network"),
        ("GET", "/diagnostics"),
        ("GET", "/system"),
        ("GET", "/about"),
        ("POST", "/sources/refresh"),
        ("POST", "/playback/clear"),
        ("POST", "/system/reboot"),
        ("POST", "/logout"),
    ],
)
def test_unauthenticated_routes_redirect_to_login(method, path):
    application = build_app(services=preview_services("anon-guard"))
    client = application.test_client()
    response = client.open(path, method=method)
    assert response.status_code == 302
    assert response.headers["Location"].endswith("/login")


def test_csrf_protected_mutation_succeeds_with_token():
    services = preview_services("csrf-ok")
    application = build_app(services=services, WTF_CSRF_ENABLED=True)
    client = application.test_client()
    login_page = client.get("/login").get_data(as_text=True)
    login_token = re.search(r'name="csrf_token"[^>]*value="([^"]+)"', login_page)
    assert login_token is not None
    assert (
        client.post(
            "/login",
            data={"password": "csrf-ok", "csrf_token": login_token.group(1)},
        ).status_code
        == 302
    )
    dashboard = client.get("/").get_data(as_text=True)
    match = re.search(r'name="csrf_token"[^>]*value="([^"]+)"', dashboard)
    assert match is not None
    response = client.post(
        "/sources/refresh",
        data={"csrf_token": match.group(1)},
        follow_redirects=True,
    )
    assert response.status_code == 200
    assert b"Session expired" not in response.data


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
