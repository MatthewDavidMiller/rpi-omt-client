"""Dashboard and playback-control routes."""

from __future__ import annotations

from flask import Blueprint, redirect, render_template, request, url_for
from flask.typing import ResponseReturnValue

from .common import login_required, publish_action, services

dashboard_blueprint = Blueprint("dashboard", __name__)


@dashboard_blueprint.get("/")
@login_required
def dashboard() -> ResponseReturnValue:
    source = services().source
    available_sources = source.sources()
    playback = source.playback()
    return render_template(
        "dashboard.html",
        sources=available_sources,
        current_source=playback.source,
        current_direct_target=playback.direct_address,
        playback=playback,
    )


@dashboard_blueprint.post("/sources/select")
@login_required
def select_source() -> ResponseReturnValue:
    publish_action(services().source.select(request.form.get("source", "")))
    return redirect(url_for("dashboard.dashboard"))


@dashboard_blueprint.post("/sources/refresh")
@login_required
def refresh_sources() -> ResponseReturnValue:
    services().source.refresh()
    return redirect(url_for("dashboard.dashboard"))


@dashboard_blueprint.post("/playback/restart")
@login_required
def restart_playback() -> ResponseReturnValue:
    publish_action(services().source.restart())
    return redirect(url_for("dashboard.dashboard"))


@dashboard_blueprint.post("/playback/clear")
@login_required
def clear_playback() -> ResponseReturnValue:
    publish_action(services().source.clear())
    return redirect(url_for("dashboard.dashboard"))
