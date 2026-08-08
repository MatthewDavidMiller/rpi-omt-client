"""Flask application factory."""

from __future__ import annotations

import socket
from datetime import timedelta
from pathlib import Path
from typing import Any, cast

from flask import Flask, render_template, request
from flask.typing import ResponseReturnValue, RouteCallable
from flask_limiter import Limiter
from flask_limiter.util import get_remote_address
from flask_wtf import CSRFProtect
from flask_wtf.csrf import CSRFError
from werkzeug.exceptions import HTTPException
from werkzeug.wrappers import Response

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
        MAX_CONTENT_LENGTH=effective_settings.max_request_bytes,
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

    def apply_limit(endpoint: str, limit: str, **options: Any) -> None:
        """Attach a settings-driven rate limit after blueprint registration.

        The limits come from `AppSettings`, so they cannot be spelled as import-time
        decorators on the views themselves. Flask-Limiter's decorator is typed for
        both sync and async views while `view_functions` is sync-only, hence the
        narrowing cast. `tests/unit/test_factory_routes.py` asserts every endpoint
        below actually returns 429 at its configured limit.
        """
        limited = limiter.limit(limit, **options)(app.view_functions[endpoint])
        app.view_functions[endpoint] = cast(RouteCallable, limited)

    apply_limit("auth.login", effective_settings.login_rate_limit, methods=["POST"])
    for endpoint in (
        "diagnostics.discovery_check",
        "diagnostics.runtime_check",
        "diagnostics.direct_check",
    ):
        apply_limit(endpoint, effective_settings.diagnostics_action_limit)
    apply_limit("diagnostics.download_bundle", effective_settings.diagnostics_download_limit)
    apply_limit("system.reboot", effective_settings.reboot_action_limit)

    @app.context_processor
    def shared_template_context() -> dict[str, object]:
        return {
            "hostname": socket.gethostname(),
            "authenticated": authenticated(),
        }

    @app.after_request
    def security_headers(response: Response) -> Response:
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

    def contextual_error(title: str, message: str, status: int) -> ResponseReturnValue:
        if authenticated():
            return render_template("error.html", title=title, message=message), status
        return render_template("login.html", error=message), status

    @app.errorhandler(CSRFError)
    def csrf_error(_error: CSRFError) -> ResponseReturnValue:
        return contextual_error("Session expired", "Session expired. Please try again.", 400)

    @app.errorhandler(413)
    def request_too_large(_error: Exception) -> ResponseReturnValue:
        return contextual_error("Request too large", "Request is too large.", 413)

    @app.errorhandler(404)
    def not_found(_error: Exception) -> ResponseReturnValue:
        return contextual_error("Page not found", "That page does not exist.", 404)

    @app.errorhandler(405)
    def method_not_allowed(_error: Exception) -> ResponseReturnValue:
        return contextual_error(
            "Action not allowed",
            "That action is not allowed on this page.",
            405,
        )

    @app.errorhandler(500)
    @app.errorhandler(Exception)
    def internal_error(error: Exception) -> ResponseReturnValue:
        """Render an operator-facing page for any unhandled failure.

        Werkzeug's own HTTP errors keep their status and message; everything else
        is logged with a traceback and reported as an opaque 500, so an
        exception string never reaches the browser. Flask does not re-enter this
        handler when rendering fails, so the template call is guarded locally.
        """
        if isinstance(error, HTTPException) and error.code is not None:
            try:
                return contextual_error(
                    error.name,
                    error.description or "The request could not be completed.",
                    error.code,
                )
            except Exception:
                app.logger.exception("Failed to render HTTP error page for %s", request.path)
                return (error.description or error.name, error.code)
        app.logger.exception("Unhandled error serving %s", request.path)
        try:
            return contextual_error(
                "Something went wrong",
                "The appliance could not complete that request. Check the container logs.",
                500,
            )
        except Exception:
            app.logger.exception("Failed to render the operator error page for %s", request.path)
            return ("Internal Server Error", 500)

    @app.errorhandler(429)
    def rate_limited(_error: Exception) -> ResponseReturnValue:
        if request.endpoint == "auth.login" or not authenticated():
            return render_template("login.html", error="Too many login attempts. Please wait."), 429
        return render_template(
            "error.html",
            title="Too many requests",
            message="Too many requests. Please wait and try again.",
        ), 429

    return app
