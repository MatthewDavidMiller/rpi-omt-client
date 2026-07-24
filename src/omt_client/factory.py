"""Flask application factory."""

from __future__ import annotations

import socket
from datetime import timedelta
from pathlib import Path

from flask import Flask, render_template, request, session
from flask_limiter import Limiter
from flask_limiter.util import get_remote_address
from flask_wtf import CSRFProtect
from flask_wtf.csrf import CSRFError

from .routes import (
    about_blueprint,
    auth_blueprint,
    dashboard_blueprint,
    diagnostics_blueprint,
    network_blueprint,
    system_blueprint,
)
from .routes.common import authenticated
from .services import ServiceContainer, production_services
from .settings import AppSettings, load_settings

MAX_REQUEST_BYTES = 16 * 1024


def create_app(
    settings: AppSettings | None = None,
    services: ServiceContainer | None = None,
) -> Flask:
    """Create one configured web application with explicit service dependencies."""
    effective_settings = settings or load_settings()
    effective_services = services or production_services(effective_settings)
    asset_root = Path(__file__).resolve().parent
    app = Flask(
        "omt_client",
        template_folder=str(asset_root / "templates"),
        static_folder=str(asset_root / "static"),
        static_url_path="/static",
    )
    app.secret_key = effective_services.auth.secret_key
    app.config.update(
        WTF_CSRF_ENABLED=True,
        SESSION_COOKIE_SECURE=True,
        SESSION_COOKIE_HTTPONLY=True,
        SESSION_COOKIE_SAMESITE="Lax",
        MAX_CONTENT_LENGTH=MAX_REQUEST_BYTES,
        PERMANENT_SESSION_LIFETIME=timedelta(seconds=effective_settings.session_lifetime_seconds),
    )
    app.extensions["omt_client.settings"] = effective_settings
    app.extensions["omt_client.services"] = effective_services

    csrf = CSRFProtect(app)
    limiter = Limiter(
        get_remote_address,
        app=app,
        default_limits=[],
        storage_uri="memory://",
    )
    app.extensions["omt_client.csrf"] = csrf
    app.extensions["omt_client.limiter"] = limiter

    for blueprint in (
        auth_blueprint,
        dashboard_blueprint,
        network_blueprint,
        diagnostics_blueprint,
        system_blueprint,
        about_blueprint,
    ):
        app.register_blueprint(blueprint)

    app.view_functions["auth.login"] = limiter.limit("5 per minute", methods=["POST"])(
        app.view_functions["auth.login"]
    )
    for endpoint in (
        "diagnostics.discovery_check",
        "diagnostics.runtime_check",
        "diagnostics.direct_check",
    ):
        app.view_functions[endpoint] = limiter.limit(effective_settings.diagnostics_action_limit)(
            app.view_functions[endpoint]
        )
    app.view_functions["diagnostics.download_bundle"] = limiter.limit(
        effective_settings.diagnostics_download_limit
    )(app.view_functions["diagnostics.download_bundle"])
    app.view_functions["system.reboot"] = limiter.limit(effective_settings.reboot_action_limit)(
        app.view_functions["system.reboot"]
    )

    @app.context_processor
    def shared_template_context():
        return {
            "hostname": socket.gethostname(),
            "authenticated": bool(session.get("authenticated")),
        }

    @app.after_request
    def security_headers(response):
        response.headers["Strict-Transport-Security"] = "max-age=31536000; includeSubDomains"
        response.headers["X-Frame-Options"] = "DENY"
        response.headers["X-Content-Type-Options"] = "nosniff"
        response.headers["Referrer-Policy"] = "strict-origin-when-cross-origin"
        response.headers["Content-Security-Policy"] = (
            "default-src 'self'; style-src 'self'; script-src 'none'; form-action 'self'"
        )
        if request.endpoint != "static":
            response.headers["Cache-Control"] = "no-store"
            response.headers["Pragma"] = "no-cache"
        return response

    def contextual_error(title: str, message: str, status: int):
        if authenticated():
            return render_template("error.html", title=title, message=message), status
        return render_template("login.html", error=message), status

    @app.errorhandler(CSRFError)
    def csrf_error(_error):
        return contextual_error("Session expired", "Session expired. Please try again.", 400)

    @app.errorhandler(413)
    def request_too_large(_error):
        return contextual_error("Request too large", "Request is too large.", 413)

    @app.errorhandler(429)
    def rate_limited(_error):
        if request.endpoint == "auth.login" or not authenticated():
            return render_template("login.html", error="Too many login attempts. Please wait."), 429
        return render_template(
            "error.html",
            title="Too many requests",
            message="Too many requests. Please wait and try again.",
        ), 429

    return app
