"""Authentication routes."""

from __future__ import annotations

from flask import Blueprint, current_app, redirect, render_template, request, session, url_for
from flask.typing import ResponseReturnValue

from .common import authenticated, services

auth_blueprint = Blueprint("auth", __name__)


@auth_blueprint.route("/login", methods=["GET", "POST"])
def login() -> ResponseReturnValue:
    if request.method == "POST":
        # Read the body outside the guard below. Parsing it enforces
        # MAX_CONTENT_LENGTH, and that RequestEntityTooLarge must reach Flask's
        # 413 handler instead of being reported as a storage failure.
        submitted_password = request.form.get("password", "")
        previous = session.get("session_id")
        previous_session_id = previous if isinstance(previous, str) else None
        try:
            session_id = services().auth.authenticate(submitted_password, previous_session_id)
        except Exception:
            current_app.logger.exception("Unable to create a persistent web session")
            return render_template(
                "login.html",
                error="Unable to create a persistent session. Check configuration storage.",
            ), 503
        if session_id:
            session.clear()
            session["authenticated"] = True
            session["session_id"] = session_id
            session["password_digest"] = services().auth.password_digest
            session.permanent = True
            return redirect(url_for("dashboard.dashboard"))
        return render_template("login.html", error="Invalid password")
    if authenticated():
        return redirect(url_for("dashboard.dashboard"))
    return render_template("login.html")


@auth_blueprint.post("/logout")
def logout() -> ResponseReturnValue:
    stored = session.get("session_id")
    session_id = stored if isinstance(stored, str) else None
    try:
        services().auth.revoke(session_id)
    except Exception:
        current_app.logger.exception("Unable to revoke the persistent web session")
        # Clear the signed browser cookie even when the durable registry is
        # unavailable. Leaving the local session authenticated after an
        # explicit logout request is the least safe possible fallback.
        session.clear()
        return render_template(
            "login.html",
            error=(
                "Signed out locally, but the server session could not be revoked. "
                "Close this browser and check configuration storage."
            ),
        ), 503
    session.clear()
    return redirect(url_for("auth.login"))
